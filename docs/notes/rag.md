# Archivo de conocimiento local (RAG)

Estado: **cableado** en servidor e IA (apagado por defecto: `RAG_ENABLED=0`). Todo está probado con el embedder simulado;
**ningún embedding real se ha calculado todavía**: no se usó Ollama ni la GPU y no se descargó ningún modelo (ver «Cableado»).

## Principio
El LLM no aprende: consulta. El archivo guarda texto que la app ya tiene (noticias descargadas, secciones de
folletos de OPV, análisis pasados), lo trocea, calcula un vector por fragmento y, al preguntar, devuelve los
fragmentos más relevantes **con fuente, fecha y URL**. El modelo los recibe numerados `[D1]…[Dn]`, igual que las
noticias `[N#]`, y sus citas se verifican en Rust contra lo que realmente se le entregó. Es la misma filosofía que
`docs/ARCHITECTURE.md` («por qué no se hace fine-tuning»): cifras verificables y con fecha, no memoria del modelo.

## Qué se guarda
| Tipo (`DocKind`) | Origen | Qué se indexa | Clave lógica (`doc_key`) |
|---|---|---|---|
| `news` | `NewsItem` que `market` ya devolvió (sin peticiones extra) | título + resumen | `news:<NewsItem.id>` |
| `filing` | S-1/F-1/424B… que obtiene el otro agente, por secciones | cada sección troceada | `filing:<accession>` |
| `analysis` | `Analysis` guardado (más adelante, con su resultado en `meta`) | tesis, casos alcista/bajista | `analysis:<uuid>` |

Esquema (`backend/crates/server/migrations/0004_knowledge.sql`, misma BD SQLite, fechas en ms Unix):
- `documents`: `doc_key` único, `content_hash` único, `kind`, `subtype` (S-1…), `source`, `publisher`, `title`, `url`,
  `issuer`, `language`, `published_at`, `ingested_at`, `chunk_count`, `pinned`, `meta` (JSON libre).
- `document_assets(document_id, asset_id)`: un documento puede pertenecer a varios activos (`stock:AAPL`).
- `chunks`: `document_id`, `ordinal`, `section`, `text`, `text_hash`, `token_count`.
- `embeddings(chunk_id, model, dim, vec BLOB f32 LE normalizado)`: PK `(chunk_id, model)`. `model` identifica el
  ESPACIO vectorial (`modelo|estilo de prompt|dimensiones`); el índice solo carga el del modelo activo, así que
  cambiar `RAG_EMBED_MODEL` nunca mezcla espacios: los fragmentos quedan «pendientes» y `embed_pending` los recalcula.
- Los embeddings son datos **derivados** (regenerables desde `chunks.text`); el texto no.

Metadatos pedidos: fuente (`source`/`publisher`), URL, fecha (`published_at`), activo/emisor (`document_assets`/`issuer`),
tipo (`kind`+`subtype`), idioma, hash (`content_hash` del documento, `text_hash` del fragmento).

## Troceado (`chunk.rs`)
- Por **secciones** (nunca se mezcla texto de dos secciones). `sections_from_text` detecta cabeceras de folleto
  (`ITEM 1A. RISK FACTORS`, `USE OF PROCEEDS`, líneas en mayúsculas, `# …`); si el origen ya trae secciones
  estructuradas, es mejor pasarlas tal cual (las cabeceras en Title Case no se detectan).
- Dentro de cada sección: cortes en fin de **oración** (con lista de abreviaturas: `Inc.`, `U.S.`, `Dr.`, decimales),
  objetivo **400 tokens**, máximo **600**, solape de **60** tokens (las últimas oraciones del fragmento anterior), cola
  nueva < 80 tokens se funde con el anterior si cabe. Una oración > 600 se parte por palabras. Determinista.
- Tokens **estimados** a 3,5 caracteres/token (conservador para castellano y cifras). No es el tokenizador real: un
  fragmento de «600» puede ser ~700 reales; cabe de sobra en los 2048 de embeddinggemma/nomic.
- Lo que se embebe en folletos es `«Título — Sección\nTexto»` (un fragmento suelto no dice de qué empresa habla). En
  noticias el título ya es la primera línea. El texto **almacenado** es el original, sin cabecera.

## Deduplicación
1. Misma `doc_key` y mismo `content_hash` → `Unchanged` (solo se fusionan activos nuevos: la misma noticia pedida desde
   AAPL y luego desde MSFT queda enlazada a ambos).
2. Misma `doc_key`, contenido distinto → `Updated` (se reemplazan sus fragmentos).
3. Otra `doc_key` con el mismo `content_hash` → `Duplicate`. Noticias: hash = título sin « - Medio» + día (colapsa el
   mismo titular de Yahoo y Google News); folletos: texto normalizado completo.
4. **Fragmentos idénticos entre documentos** (un S-1/A repite casi todo el S-1): se reutiliza el embedding por
   `text_hash` (no se recalcula) y en los resultados se colapsan (gana el mejor puntuado).

## Recuperación
1. Se embebe la consulta (`EmbedKind::Query`; cada familia de modelos usa su prefijo, ver abajo).
2. **Prefiltro** en memoria por metadatos: activos (cualquiera de; `include_general` añade los documentos sin activo),
   tipos, emisor, rango de fechas. Con filtro por activo el escaneo baja de 30 ms a ~0,3 ms (medido).
3. Similitud coseno exacta (producto escalar, vectores normalizados) → `5k` candidatos (mín. 25).
4. **Reranking** simple: `score = 0,80·sim + 0,12·recencia + 0,08·peso_fuente`.
   - recencia = `0,5^(edad/vida_media)`: noticias 14 d, folletos 365 d, análisis 90 d.
   - peso de fuente: folleto/`sec` 1,0; noticia 0,75; `google_news` 0,6 (agregador); análisis propio 0,6 (no es fuente
     primaria). Configurable en `KnowledgeConfig::source_weights`. Son **heurísticas razonadas, no calibradas**: sin
     un conjunto de evaluación propio no puedo decir que 0,80/0,12/0,08 sea óptimo; la similitud domina a propósito
     para que la recencia desempate y no sustituya a la relevancia.
5. Colapso de texto idéntico y tope por documento (`max_per_doc`, 2) para que un S-1 largo no ocupe los `k`.
6. `min_similarity` opcional: el umbral depende del modelo, hay que calibrarlo con datos reales (no hay valor por defecto).

Si el embedder falla **y no hay búsqueda léxica**, `search` devuelve error: el llamador debe seguir **sin** conocimiento y decirlo,
no inventar. Tras un fallo transitorio hay 30 s de pausa para no martillear a Ollama (los textos quedan pendientes). Con la
híbrida (abajo) el fallo del embedder degrada a solo BM25 en vez de devolver error.

### Búsqueda híbrida (FTS5/BM25 + vectores, `RAG_HYBRID`, activada por defecto)
Los embeddings flojean con identificadores exactos (tickers como `BRK.B`, «Item 1A», cifras); BM25 los encuentra tal cual y
funciona sin Ollama. Código: `lexical.rs` (saneado y RRF), `store.rs` (tabla y consulta), `kb.rs` (fusión).
- **Tabla FTS5 perezosa, sin migración**: `chunks_fts` se crea al abrir (`CREATE VIRTUAL TABLE IF NOT EXISTS`) con columnas
  `text, title, section`, **sin contenido propio** (`content='', contentless_delete=1`: el texto sigue solo en `chunks`; borrar por
  `rowid` exige SQLite ≥ 3.43, hay 3.46) y `tokenize='unicode61 remove_diacritics 2'` (parte por no alfanuméricos y quita tildes: sirve a
  español e inglés; `porter` descartado, es solo inglés). `rowid` = `chunks.id`. Si crear o sondear la tabla falla, `tracing::warn!` y el
  archivo funciona **solo con vectores** (`Stats.lexical = false`); el arranque nunca se rompe. Con `RAG_HYBRID=0` ni se crea ni se toca.
- **Relleno**: los fragmentos anteriores se indexan en segundo plano, en tandas de 500, cediendo la puerta de escritura entre tandas
  (`backfill_lexical`, idempotente). Si el recuento de la tabla coincide con el de `chunks` no hace nada (reinicio normal). Mientras
  tanto la parte léxica devuelve resultados parciales (la vectorial no cambia). Al abrir se borran huérfanos de una sesión que cayó a mitad.
- **Coherencia**: altas y bajas son **explícitas dentro de la misma transacción** (ingesta, reemplazo, `delete_by_key`, `purge`), **no con
  triggers**: un trigger sobre `chunks` seguiría en la BD aunque FTS5 dejara de existir y haría fallar cada alta o baja de fragmentos.
  Si una escritura FTS5 falla durante la ingesta, el documento NO se pierde: se desactiva la parte léxica hasta reiniciar y el próximo
  arranque la repara (`backfill`).
- **Consulta**: `lexical::fts_query` pone **cada término entre comillas dobles** (una cadena entre comillas es siempre una frase literal:
  `AND/OR/NOT/NEAR`, `*`, `^`, `col:`, `-`, `(` pierden su significado), elimina comillas internas y caracteres de control, descarta
  términos sin alfanuméricos y palabras vacías es/en, deduplica, y limita a 300 caracteres y 16 términos; se unen con `OR` (BM25 ordena
  por cuántos y cuán raros). `BRK.B` -> `"BRK.B"` (el tokenizador lo parte en `brk b` adyacentes). Sin términos útiles no hay parte léxica.
- **Filtros**: tipo, fechas y activo se aplican en SQL (si no, con un activo concreto el top global podría no contener ninguno suyo);
  el emisor se aplica después con los metadatos en memoria. `min_similarity` solo afecta a la parte vectorial.
- **Fusión**: Reciprocal Rank Fusion con k = 60 (valor del artículo original; **no hay datos propios para ajustarlo**) sobre las dos listas de
  `5k` candidatos (mín. 25): `rrf = Σ 1/(60 + posición)`. Se normaliza por el máximo posible (`2/61`) y **sustituye a la similitud** en
  la fórmula de reranking heurístico, que sigue igual (`0,80·relevancia + 0,12·recencia + 0,08·fuente`) y después colapso y tope por
  documento. Sin candidatos léxicos la puntuación es exactamente la de antes. En `SearchHit`, `similarity` es el coseno real (0 si el
  fragmento solo salió de BM25) y `lexical` indica si BM25 también lo devolvió. BM25 pondera `text`/`title`/`section` 1,0/1,5/1,5 (heurística).
- **Sin embedder**: si Ollama está caído o el fragmento aún no tiene vector, la parte léxica sigue; un fragmento sin embedding antes no se
  recuperaba y ahora sí. Una consulta sin términos léxicos útiles y sin embedder sigue devolviendo error.
- **Latencia léxica medida** (`cargo run --release -p knowledge --example bench_lexical -- 25000`; CPU, sin embeddings; ~1 KB por fragmento,
  vocabulario sintético de 30 000 palabras con distribución sesgada; 30 repeticiones; BD en disco con caché del SO caliente):

| Fragmentos | Relleno del índice | BD (sin / con FTS5) | ticker raro p50 | 6 términos p50 / p95 | 3 muy frecuentes p50 / p95 | 6 térm. + activo p50 | 3 frec. + activo p50 |
|---|---|---|---|---|---|---|---|
| 50 000 | 5,4 s | 102 → 125 MiB (+24) | 0,13 ms | 9,8 / 11,9 ms | 27,7 / 32,8 ms | 6,2 ms | 18,5 ms |
| 100 000 (tope `RAG_MAX_CHUNKS`) | 11,8 s | 203 → 248 MiB (+45) | 0,08 ms | 17,2 / 24,0 ms | 55,4 / 65,1 ms | 11,2 ms | 37,4 ms |

  Lectura honesta: es **lineal con el número de fragmentos que contienen los términos**, no con el total; las palabras muy frecuentes son
  el peor caso (aquí con un vocabulario sintético, no con texto real; no se ha medido con noticias reales). Las palabras vacías ya se
  descartan en la consulta. Sumado a ~25–60 ms de la parte vectorial (extrapolación) queda por debajo de 130 ms en el tope; ante
  un embedder de ~100+ ms por consulta es secundario. El índice FTS5 añade ~0,45 MiB por cada 1 000 fragmentos.

## Citas `[D#]` y verificación (`citation.rs`)
- `Citations::new(hits)` numera `D1…Dn` por orden de puntuación (por fragmento, no por documento).
- `render(lang)` produce el bloque para el prompt: `<knowledge_base>` + una entrada por fragmento con tipo, fuente, fecha,
  emisor, sección, URL y el texto en una línea. `source_refs(tags)` da `domain::SourceRef` para `Analysis.sources`.
- `clean(text)` revisa cada `[D#]` de la salida: etiqueta inexistente → se quita; la frase no comparte **ninguna cifra ni
  2 palabras de contenido** con el fragmento → se quita (misma heurística que `Verifier::clean_citations` para `[N#]`).
  Devuelve `CitationIssue::{Unknown, Unsupported}` para `qualityNotes`.
- Es una heurística léxica: detecta citas inventadas o sin relación, **no** garantiza que la frase sea fiel (una frase
  que reutiliza dos palabras del fragmento pero afirma otra cosa pasa). Para cifras, lo robusto es que `ai` meta el texto
  de los fragmentos en su `NumberIndex` (ver coordinador).

## Prompt injection
Los fragmentos son datos no fiables (un titular o un folleto pueden decir «ignora las instrucciones anteriores»).
- **Se guardan y recuperan sin alterar un carácter** (alterarlos falsearía la evidencia y daría falsa seguridad). Solo
  se registra un `warn` y `IngestReport.suspicious` si coinciden patrones conocidos (`looks_like_injection`); no cambia el ranking.
- Al **renderizar** se aplica la misma política auditada que `ai::safety::sanitize_untrusted`: una línea, sin `<>` (ni
  `<|…|>`), sin `[]` (no pueden falsificar `[D2]` ni cerrar `</knowledge_base>`), sin vallas ni caracteres invisibles. Los
  metadatos (título, sección, URL, emisor) pasan por lo mismo.
- El bloque lleva una cabecera que lo declara dato; el system prompt de `ai` debe repetirlo (ver coordinador).
- El texto de un fragmento no puede validar una cita: `clean` solo mira la salida del modelo (test
  `a_fragment_cannot_vouch_for_other_tags`). Las herramientas de `ai` son de solo lectura (S-10), así que una inyección
  no puede actuar; el riesgo residual es que el 7B repita una orden o una afirmación falsa del fragmento.
- Riesgo propio de los folletos: son texto promocional del emisor. El peso de fuente 1,0 significa «primaria y
  verificable», no «imparcial»; el prompt debe tratarlos como declaraciones de la empresa.

## Índice vectorial: por qué en memoria y no `sqlite-vec`
**Decisión: matriz plana `Vec<f32>` en RAM cargada desde SQLite y búsqueda exacta por fuerza bruta**
(`index.rs`; el contrato `Embedder`/`KnowledgeBase` no depende del índice, se puede cambiar sin tocar nada más).
Medido (`cargo run --release --example bench_index -- 50000 256 768 1024`, CPU de este equipo, ablación de GPU en
marcha pero esto no usa GPU; 30 búsquedas top-30, mediana):

| 50 000 vectores | RAM de vectores (teórica) | RSS añadido (medido) | top-30 sin filtro p50 / máx | con filtro por activo p50 |
|---|---|---|---|---|
| 256 dim | 48,8 MiB | 54,1 MiB | 6,6 / 10,1 ms | 0,21 ms |
| 768 dim | 146,5 MiB | 150,6 MiB | 22,7 / 25,2 ms | 0,29 ms |
| 1024 dim | 195,3 MiB | 200,1 MiB | 30,3 / 32,7 ms | 0,32 ms |

Por fragmento: `4·dim` bytes + 32 B de fila (+ metadatos por documento). Con el tope por defecto de 100 000 fragmentos:
~300 MiB a 768 dim (~400 MiB a 1024), latencia ~45–60 ms sin filtro (extrapolación lineal, no medida). En disco, cada
fragmento ocupa ~`4·dim` + ~1,4 KB de texto (estimado): 100 000 ≈ 450 MB en SQLite (+ WAL). Los embeddings no
compiten con la VRAM: el índice está en RAM del sistema (31 GB en este equipo).
Arranque: leer ~150 MB de BLOBs de SQLite y construir el índice; no medido con Ollama pero es lectura local.

`sqlite-vec` **no aporta ahora**: (1) su README advierte «pre-v1, expect breaking changes»; (2) se compila con C y se
registra con `sqlite3_auto_extension` + `transmute` (`unsafe`), y su guía de Rust solo cubre rusqlite, no `sqlx`
(no lo he probado); (3) con metadatos, su propia documentación habla de «slower full scan»; la documentación que leí
no dice si `vec0` es exacto o aproximado, así que **no puedo afirmar que sea más rápido** que 23 ms para 50 000×768, y
una búsqueda exacta dentro de SQLite no puede bajar de ese orden. Revisarlo si algún día pasamos de ~500 k vectores
o de ~1 GB de RAM.

## Modelos de embeddings en Ollama
Datos de **manifest del registro** (`registry.ollama.ai`, tamaño de la capa del modelo) y **cabecera GGUF** leída con
una petición `Range` de 128 KiB por modelo (dimensión, contexto, capas; no se descargó ningún modelo), más las fichas de
Hugging Face. Puntuaciones MTEB: de las fichas oficiales de cada modelo (autodeclaradas por sus autores).

| Modelo (tag) | Tamaño real | Params | Dim | Contexto | Idiomas | MTEB multiling. v2 (media tareas) | Prefijos |
|---|---|---|---|---|---|---|---|
| `nomic-embed-text` | 274,3 MB (F16) | 137 M | 768 (MRL 512/256/128/64) | 2048 en GGUF (params: `num_ctx` 8192) | etiqueta de la ficha: solo inglés | — | `search_document:` / `search_query:` |
| `embeddinggemma:300m-qat-q8_0` | **338,0 MB** | 307,6 M | 768 (MRL 512/256/128) | 2048 | 100+ | **60,93** | `title: none \| text:` / `task: search result \| query:` |
| `embeddinggemma:300m-qat-q4_0` | 238,5 MB | 307,6 M | 768 | 2048 | 100+ | 60,62 | ídem |
| `embeddinggemma` (latest = BF16) | 621,9 MB | 307,6 M | 768 | 2048 | 100+ | 61,15 | ídem |
| `qwen3-embedding:0.6b` (Q8_0) | **639,2 MB** | 595,8 M | 1024 (MRL 32–1024) | 32 768 | 100+ | **64,33** (inglés v2: 70,70) | consulta: `Instruct: …\nQuery:`; documento: ninguno |
| `qwen3-embedding:4b` (Q4_K_M) | 2496,7 MB | 4,0 B | 2560 | 40 960 | 100+ | 69,45 | ídem |
| `qwen3-embedding` (latest = 8b Q4_K_M) | 4676,8 MB | 7,6 B | 4096 | — | 100+ | 70,58 | ídem |
| `bge-m3` (solo F16) | 1157,7 MB | 566,7 M | 1024 | 8192 | 100+ | 59,56 | ninguno |
| `snowflake-arctic-embed2` | 1160,3 MB | 566,7 M | 1024 | 8192 | 74 (lista en el GGUF) | no consultado | — |
| `paraphrase-multilingual` | 562,8 MB | 277 M | 768 | 512 | ~50 | no es de recuperación | — |
| `mxbai-embed-large` / `all-minilm` / `granite-embedding` | 669,6 / 45,9 / 62,5 MB | 334 / 23 / 30 M | 1024 / 384 / 384 | 512 | inglés | — | — |

(Las 59,56 de bge-m3 y 64,33 de Qwen3-0.6B salen de la misma tabla del repositorio de Qwen; la de embeddinggemma, de su
ficha. Mismo benchmark nominal, pero fuentes distintas y media de todas las tareas, no solo recuperación.)

Criterio: la app mezcla **consultas en español** con **folletos y noticias en inglés** (S-1/F-1, Yahoo) y noticias en
español (Google News `es`), así que hace falta recuperación **entre idiomas**. Eso descarta `nomic-embed-text`,
`all-minilm`, `mxbai` y `granite` (inglés). Contexto de 2048 sobra para fragmentos de ≤ 600 tokens.

VRAM (RTX 4070 de 8 GB con el LLM cargado: `nvidia-smi` marcaba 5623/8188 MiB en el momento de consultar, con más carga
de otros agentes): caché KV por token calculada de la cabecera GGUF = `2·capas·kv_heads·head_dim·2 B`:
embeddinggemma 24·1·256 → 24 576 B/token (~48 MiB a 2048); Qwen3-0.6B 28·8·128 → 114 688 B/token (~448 MiB a 4096
tokens; el `num_ctx` real que usa Ollama para embeddings **no lo he verificado**). Pesos: 338 MB frente a 639 MB.

### Recomendación (provisional, con datos de fichas; falta medirla en nuestro corpus)
**`RAG_EMBED_MODEL=embeddinggemma:300m-qat-q8_0`**: es multilingüe, el más pequeño de los multilingües (338 MB, ~48 MiB
de KV), 768 dim (la mitad de memoria por vector que 1536) y la cuantización Q8 pierde 0,22 puntos MTEB frente a la
precisión completa según su ficha. Es el que menos disputa VRAM al LLM de 7B.
**Alternativa: `qwen3-embedding:0.6b`**: +3,4 puntos MTEB multilingüe (64,33 frente a 60,93), Apache-2.0 (Gemma tiene sus
propios términos de uso), pero ~2× de pesos, ~2× de cómputo y bastante más KV. Sería la elección si la medida real
muestra que embeddinggemma falla en el cruce es↔en.
No recomiendo `bge-m3` (1,16 GB solo en F16, puntuación inferior) ni `qwen3-embedding:latest` (4,7 GB).

**Qué NO está verificado** (la GPU estaba ocupada): velocidad real en esta máquina (chunks/s en GPU y CPU),
que Ollama 0.40.3 sirva bien los tags `qat-q8_0` y `qwen3-embedding:0.6b`, si competir con el LLM por VRAM provoca
descargas/recargas del modelo de chat, y la calidad real es↔en sobre nuestros textos. Hay un comentario de terceros sobre
un posible deterioro de calidad con lotes grandes (issue #6262 de Ollama) que **no pude verificar**; por eso el lote por
defecto es 16 y conviene comprobar que `embed([a,b,c])` ≈ `embed([a])`.
**Protocolo para decidir** cuando haya GPU libre (necesita aprobación para `ollama pull`): 1) descargar ambos; 2) 40
pares (consulta es/en → pasaje en/es) sacados de folletos y titulares reales; 3) medir recall@5, MRR, ms/fragmento y
VRAM (`ollama ps`) con LLM cargado; 4) elegir el menor que no pierda > 5 puntos de recall@5.
Notas de uso: `RAG_EMBED_DIMS=512` trunca (Matryoshka) y ahorra 33 % de RAM por -0,44 MTEB en gemma (ficha); `RAG_EMBED_NUM_GPU=0`
pide a Ollama CPU para no competir por VRAM (opción de Ollama, **sin verificar** su efecto con estos modelos).

## Retención y tamaño
- Noticias 90 días (`RAG_NEWS_RETENTION_DAYS`), folletos 1095 (3 años), análisis sin caducidad (0).
- Tope duro `RAG_MAX_CHUNKS` = 100 000. Al superarlo `purge` expulsa primero noticias antiguas, luego folletos antiguos;
  **nunca** análisis ni documentos `pinned` (p. ej. los citados por un análisis guardado, `KnowledgeBase::pin`).
- Estimación: unas 600 noticias/día con 20 activos seguidos × 90 d ≈ 54 000 fragmentos (~165 MiB a 768 dim); 20 OPV
  con secciones clave ≈ 6 000 fragmentos. Es una estimación de volumen, no una medida: `stats()` da las cifras reales.
- `purge(now)` hay que llamarla periódicamente (cada pocas horas); el crate no lanza temporizadores propios.
- Los embeddings viven en la misma SQLite que las conversaciones: la BD crece cientos de MB. Si molesta, mover a un
  fichero propio es sencillo (misma migración, otro pool).

## API pública (`knowledge`)
```rust
KnowledgeConfig::from_env() / from_lookup(..)            // RAG_*; enabled=false por defecto
OllamaEmbedder::new(cfg.embed.clone())?                   // impl Embedder sobre POST /api/embed (no descarga modelos)
FakeEmbedder { dim }                                      // determinista, para tests y demo (no semántico)
let kb = KnowledgeBase::open(pool, Arc<dyn Embedder>, cfg).await?;   // comprueba esquema, carga índice
kb.ingest(NewDocument) -> IngestReport                    // Added | Updated | Unchanged | Duplicate{of}
kb.ingest_news(Option<&AssetId>, &[NewsItem]) -> BatchReport
kb.search(&SearchRequest) -> SearchResults               // hits con texto original + DocumentInfo + score/sim/recencia
kb.embed_pending(limit) / kb.purge(now) / kb.pin(id, bool) / kb.merge_meta(id, json) / kb.delete_by_key(key) / kb.stats()
trait KnowledgeSearch { async fn search(&self, &SearchRequest) }   // contrato mínimo para `ai`
spawn_news_worker(kb, CancellationToken) -> (NewsSink, JoinHandle) // sink.offer(asset, &[NewsItem]): síncrono, no bloquea
sections_from_text(text) -> Vec<Section>; chunk_sections(..)
Citations::new(hits).render("es") / .clean(text) / .source_refs(["D1"])
```
Código: `chunk.rs` troceado · `embed.rs`/`ollama.rs` embeddings · `index.rs` búsqueda · `store.rs` SQLite · `kb.rs` núcleo ·
`ingest.rs` enganche de noticias · `citation.rs` citas y contexto seguro · `config.rs` entorno.

## Tests (sin red ni Ollama; Ollama simulado con un servidor axum local)
46 tests (34 unitarios + 12 de integración): troceado (límites, solape, no se pierde texto, secciones separadas, abreviaturas, UTF-8, oración
gigante, cola, determinismo, cabeceras SEC); normalización/hash; índice (top-k = ordenación ingenua, filtros, compactación
tras borrar, dimensión); `OllamaEmbedder` (forma exacta de la petición, lotes, prefijos por modelo, truncado Matryoshka,
404→modelo ausente, 5xx/caído→transitorio, recuento incorrecto); deduplicación (clave, hash, actualización, reutilización
de embeddings y colapso S-1/S-1/A); filtros (activo, general, tipo, fechas, emisor); ranking (relevancia, recencia,
fuente, tope por documento); persistencia en fichero y cambio de modelo (pendientes → `embed_pending`); embedder caído
(texto a salvo, búsqueda falla de forma explícita); retención, tope y `pinned`; **inyección** («ignora las instrucciones
anteriores» + `</knowledge_base>` + `[D1]` + `<|im_start|>`: se almacena y recupera íntegro, no cierra el bloque, no fabrica
citas y no altera la recuperación de los demás); enganche de noticias (deduplica ofertas repetidas, no bloquea).

Híbrida (`knowledge/tests/hybrid.rs`, 9 tests + 4 unitarios en `lexical.rs`): un ticker (`BRK.B`) en un fragmento largo que el vector
simulado no recupera y la híbrida pone primero; «Item 1A» (cabecera de sección); 16 consultas hostiles para FTS5 (comillas, `OR`, `NEAR(`,
`*`, `^`, `col:`, `--`, SQL, 4 000 caracteres, Unicode) sin romper la sintaxis; sin embedder (`Down`) y fragmentos sin vector se recuperan por
BM25 respetando filtros de activo y tipo; FTS5 inutilizable al abrir (tabla normal con el mismo nombre) y roto a mitad de sesión (`DROP TABLE`):
degrada a vectores sin perder el documento; borrado, actualización y purga mantienen la tabla coherente; relleno de un archivo creado con
`RAG_HYBRID=0`; `RAG_HYBRID=0` = comportamiento anterior. Los golden de `ai` con el RAG apagado no dependen de esto y siguen pasando.

## Pendiente / límites
- **Medir con Ollama real** (velocidad, VRAM, calidad es↔en, lotes) y decidir modelo con el protocolo de arriba. Sin esto
  la recomendación es de ficha técnica.
- **Búsqueda léxica (FTS5/BM25) híbrida**: los embeddings flojean con identificadores exactos (tickers, «Item 1A», cifras).
  Daría además recuperación sin Ollama. **Verificado: el SQLite que empaqueta sqlx (3.46.0) incluye FTS5**
  (`knowledge/tests/fts5.rs`). **Implementada** (ver «Búsqueda híbrida»): tabla perezosa, RRF y `RAG_HYBRID`. Falta medir con corpus y
  consultas reales si mejora el recall frente a solo vectores y ajustar k=60 y los pesos de columna; hoy solo está probado con
  el embedder simulado (que no es semántico) y con datos sintéticos.
- Sin reranker neuronal (cross-encoder): el reranking es heurístico y sin calibrar.
- Los pesos 0,80/0,12/0,08, las vidas medias y los pesos de fuente son razonados, no ajustados con datos.
- Análisis pasados: el tipo existe y el `meta` admite el resultado, pero **no hay conversión desde `Analysis`** ni lo
  recomiendo como evidencia: el modelo podría citar su propia alucinación anterior. Si se usa, solo con la etiqueta
  «análisis previo de esta app (no es fuente primaria)» (ya la lleva `render`) y para comparar «qué cambió desde entonces».
- `Citations::clean` es léxico (ver arriba). Las cifras de los fragmentos SÍ entran ya en el `NumberIndex` del análisis (solo su texto, no fecha ni URL).
- Detección de cabeceras de folleto: heurística de texto plano; HTML de EDGAR conviene seccionarlo en origen.

## Cableado (hecho)
**Cómo activarlo**: `ollama pull embeddinggemma:300m-qat-q8_0` (la app nunca descarga) y `RAG_ENABLED=1` en `.env`. Con
`INVEST_AI_FAKE=1` se ignora. `GET /api/knowledge/stats` (ver `docs/API.md`) dice si está activo y cuántos fragmentos hay.

| Pieza | Dónde |
|---|---|
| Apertura tolerante a fallos, `TeeMarket` (noticias), `TeeIpo` (folletos como `Filing` con sus secciones), mantenimiento (`embed_pending(64)` al arrancar y cada 2 min, `purge` cada 6 h), pausa por actividad | `server/src/knowledge.rs` |
| Cableado en el arranque, espera de los trabajadores en el apagado (`state.tasks`) | `server/src/main.rs`, `server/src/state.rs` |
| Ruta `GET /api/knowledge/stats` | `server/src/routes/knowledge.rs` |
| Recuperación en el análisis, reglas del system prompt, `[D#]`, fuentes | `ai/src/rag.rs`, `ai/src/analysis/mod.rs` |
| Herramienta `search_knowledge` | `ai/src/tools.rs`, `ai/src/chat.rs` |

- **Un fallo del RAG nunca impide arrancar**: si `KnowledgeBase::open` falla (p. ej. falta la migración) hay `tracing::error!` y la app
  sigue sin RAG (`knowledge: None`, `/knowledge/stats` → `enabled:false`). Si una búsqueda falla en un análisis, este sigue sin
  conocimiento y lo anota en `dataWarnings`; en el chat el error llega al modelo como resultado de la herramienta.
- **Análisis**: una búsqueda por activo antes de las etapas (`assets=[id]`, solo noticias y folletos, nunca análisis propios;
  k=6 en standard/deep, 3 en quick; tope de 20 s). El bloque `<knowledge_base>` va solo al juez. El system prompt añade la regla 7
  (dato no fiable, citar `[D#]`, los folletos son declaraciones de la empresa) **solo si se entregaron fragmentos**. Tras el juez,
  `Citations::clean` quita las citas inexistentes o sin relación (nota en `qualityNotes`) y las `[D#]` que sobreviven pasan a
  `Analysis.sources`.
- **Chat**: `search_knowledge(query, asset?, kind?, since?, limit?)`, solo lectura, `limit` ≤ 8 (def. 5), solo noticias/folletos.
  La numeración `[D#]` continúa entre llamadas del mismo turno; al final se avisa de citas `[D#]` inexistentes.
- **Pausa por VRAM**: `TrackedAnalyst` cuenta análisis y chats en curso (HTTP, SSE y lista de seguimiento, porque envuelve al analista y
  no a las rutas; los semáforos de `limits` no cubren la lista de seguimiento) y `GatedEmbedder` hace esperar a los embeddings de
  DOCUMENTOS mientras haya alguno. Las consultas no esperan (el propio análisis las necesita). Esperar y no fallar es deliberado:
  un error transitorio activaría la pausa de 30 s del archivo también para las consultas.
- **Folletos de OPV**: se archivan cuando `/api/ipo/{symbol}/prospectus` o `/risk-profile` los obtienen (decorador de `IpoData`), por
  una cola acotada de 16 y un trabajador; nunca retrasan la respuesta. Se archivan completos (antes del recorte `maxChars`), sin
  las subsecciones (el padre ya las contiene). `doc_key = filing:<accession>`.

### RAG apagado = idéntico byte a byte
`knowledge` es `None` en el motor de análisis, en el de chat y en el `ToolRunner`; el servidor no envuelve nada. Garantías
comprobadas: (1) tests `rag_off_standard_analysis_matches_golden_transcript` y `rag_off_chat_matches_golden_transcript` (en `ai`):
huella FNV-64 de TODAS las peticiones enviadas a Ollama más el `Analysis`/eventos de chat sobre el fixture, calculada con el
código anterior al RAG, antes de tocarlo (se enmascaran solo la hora y la antigüedad del dato); (2) `empty_archive_is_byte_identical_to_rag_off`:
con el RAG encendido pero sin resultados la huella es la misma; (3) `definitions()` sigue devolviendo 9 herramientas y `search_knowledge`
con el RAG apagado se rechaza como «herramienta desconocida»; (4) el servidor solo envuelve mercado/OPV/analista con el RAG abierto.

### Qué falta medir (GPU libre; nada de esto se ha ejecutado)
No hay un arnés de medición hecho; se hace con la propia app:
1. `ollama pull embeddinggemma:300m-qat-q8_0` (y `qwen3-embedding:0.6b` para comparar).
2. `RAG_ENABLED=1 RAG_EMBED_KEEP_ALIVE=2m ./invest-ai` con un LLM cargado; consultar noticias de 3–4 activos y `/api/ipo/{ticker}/prospectus` de 2 OPV.
3. Velocidad de embeddings: ver `pending` en `GET /api/knowledge/stats` bajar con el tiempo (fragmentos/s) con y sin generación en curso.
4. VRAM: `watch -n1 'ollama ps; nvidia-smi --query-gpu=memory.used --format=csv'` durante un análisis con RAG: ¿se descarga y recarga el LLM al embeber la consulta? Si sí, `RAG_EMBED_NUM_GPU=0` (CPU).
5. Calidad con y sin RAG: mismo activo y profundidad con `RAG_ENABLED=0` y `=1` (comparar citas `[D#]` válidas, cifras sin verificar en `qualityNotes`, tiempo y tokens de entrada: k=6 cuesta ~2,5 k).
6. Protocolo de recall@5 es↔en de la sección «Recomendación» (40 pares).

## Para el coordinador (histórico: ya cableado, ver arriba)
1. **Migración**: `0004_knowledge.sql` ya está en `server/migrations/`. `knowledge::SCHEMA_SQL` la incluye con `include_str!`
   (única copia); `KnowledgeBase::open` falla con un mensaje claro si no se aplicó. `Cargo.lock` cambia (paquete nuevo,
   dependencias que ya estaban en el lock).
2. **Workspace**: `crates/knowledge` añadido a `members` y `knowledge = { path = … }` a `[workspace.dependencies]`.
3. **`server/Cargo.toml`**: `knowledge.workspace = true`.
4. **`server/src/main.rs`** (tras `db::connect` y antes de `AppState::new`; con `RAG_ENABLED=1`):
```rust
let kcfg = knowledge::KnowledgeConfig::from_env();
let mut kb = None;
if kcfg.enabled && !config.fake {
    let embedder = Arc::new(knowledge::OllamaEmbedder::new(kcfg.embed.clone())?);
    let k = knowledge::KnowledgeBase::open(pool.clone(), embedder, kcfg).await?;
    let (sink, worker) = knowledge::spawn_news_worker(k.clone(), state_shutdown.clone()); // el CancellationToken de AppState
    market = Arc::new(TeeMarket::new(market, sink));      // ver abajo; ninguna petición extra a proveedores
    // tarea periódica: k.purge(Utc::now()) cada 6 h, y k.embed_pending(64) al arrancar
    kb = Some(k);
}
```
   y `AppState.knowledge: Option<Arc<KnowledgeBase>>`. Esperar al `worker` en el apagado junto a `state.tasks`.
5. **Enganche de noticias sin duplicar peticiones** (`server/src/knowledge.rs`; no lo incluí en el crate porque
   `MarketData` no tiene métodos por defecto y otro agente puede añadirle alguno: un decorador en `knowledge` rompería su compilación):
```rust
pub struct TeeMarket { inner: Arc<dyn MarketData>, sink: knowledge::NewsSink }
#[async_trait] impl MarketData for TeeMarket {
    async fn news(&self, id: Option<&AssetId>, limit: usize) -> Result<Vec<NewsItem>> {
        let r = self.inner.news(id, limit).await?; self.sink.offer(id, &r); Ok(r)
    }
    async fn snapshot(&self, id: &AssetId) -> Result<AssetSnapshot> {
        let s = self.inner.snapshot(id).await?; self.sink.offer(Some(id), &s.news); Ok(s)
    }
    // …el resto delega en `inner`
}
```
   `offer` es síncrono, descarta lo ya ofrecido sin tocar la BD y, con la cola llena, descarta contando (`dropped()`); nunca
   retrasa una respuesta. Alternativa sin decorador: una línea `sink.offer(..)` en `routes/market.rs` (2 sitios), `dashboard.rs`
   y `ai/src/tools.rs:263`. Los filings de la SEC que ya salen como `NewsItem` con `provider="sec"` se archivarían como `news`;
   el otro agente debería usar `NewDocument { kind: Filing, sections, subtype, issuer, … }` para los folletos de OPV.
6. **`ai`** (no lo toqué): (a) herramienta de chat `search_knowledge(query, asset?, kind?, since?)` sobre `KnowledgeSearch`
   (solo lectura, `k` ≤ 8); (b) en el análisis, una recuperación por activo antes del juez (`assets=[id]`, `include_general=false`,
   k=6 en standard/deep y 3–4 en quick) e inyectar `Citations::render(lang)`; cuesta ~2,5 k tokens de entrada con k=6;
   (c) system prompt: el bloque `<knowledge_base>` es dato no fiable, se cita con `[D#]`, los folletos son declaraciones de la
   empresa; (d) tras la generación, `Citations::clean` y `source_refs` hacia `Analysis.sources`; (e) meter el texto de los
   fragmentos en `NumberIndex` para verificar cifras; (f) si `search` falla, seguir sin conocimiento y anotarlo en `dataWarnings`.
7. **Ollama**: el modelo de embeddings compite por VRAM con el LLM; `RAG_EMBED_KEEP_ALIVE=2m` lo descarga pronto. Valorar
   pausar la ingesta mientras hay un análisis en curso hasta tener la medida real.
8. Aviso de colisión: si otro agente crea otra `0004_*.sql`, `sqlx::migrate!` fallará por versión repetida (el coordinador
   ya asignó 0004 a este crate y 0005 al agente de aprendizaje).

## Folletos resumidos de ETF (19-oct-2026)
- Qué: la primera vez que se pide el snapshot (o el análisis, que lo necesita) de un ETF/fondo con `RAG_ENABLED=1`, `TeeMarket` ofrece su id a `server/src/fund_docs.rs`. Un trabajador en segundo plano llama a `FundDocs::fund_prospectus` (`MarketService`, ver `notes/market.md`) y lo archiva como `NewDocument { kind: Filing, subtype: 497K|485BPOS|497 }` con 5 secciones (`Investment Objective`, `Fees and Expenses`, `Principal Investment Strategies`, `Principal Risks`, `Portfolio Turnover`), `doc_key = etf:<TICKER>` (el del año siguiente sustituye al anterior), `assets = [etf:<TICKER>]` y la URL del documento en sec.gov.
- Cola acotada (8) con `try_send`: nunca retrasa una respuesta. Sin RAG (o con `INVEST_AI_FAKE=1`) no se crea nada ni se pide nada a EDGAR. En memoria no se reintenta el mismo ETF antes de 6 h (éxito o fallo); en disco, como mucho una descarga cada 30 días, incluso tras reiniciar (se mira `ingested_at` y `meta.checkedAt` del documento; un folleto sin cambios solo actualiza `checkedAt`). Tiempo máximo de una descarga: 150 s.
- Gestión del embedder igual que el resto: la ingesta espera si hay un análisis en curso.
- Tests: `server/tests/fund_docs.rs` (embedder simulado; 497K real de TLT), `market/tests/fund_prospectus.rs`.
- No medido: ingesta con Ollama real (no se usó).
