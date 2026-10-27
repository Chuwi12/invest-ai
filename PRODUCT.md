# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

- **Quien la usa:** un inversor particular que la ejecuta en su propio ordenador (un solo usuario, sin cuentas). Abre la app
  desde el menú del escritorio o el navegador para analizar un activo concreto, preguntar al chat, revisar su lista de
  seguimiento y ver si los análisis pasados acertaron.
- **Quien la ve en la landing:** terceros a los que el autor enseña el producto (portfolio, demos, contactos de BIP). Tienen
  que entender en poco tiempo qué es, cómo funciona y por qué es distinto, y entrar a la app.

## Product Purpose

Analiza en tiempo real si un activo (acciones, ETF, cripto, bonos, índices, divisas, materias primas, futuros y opciones)
es buena inversión, con datos en vivo, histórico, fundamentales y noticias, y un LLM local que justifica cada conclusión con
fuentes. El éxito es un análisis que se puede comprobar: cada cifra tiene origen y nada se inventa.

## Positioning

Todo corre en la máquina del usuario: el LLM es local (Ollama, elegido según su hardware), el servidor solo escucha en
loopback y no hay nube ni cuentas. Las cifras derivadas las calcula el código (Rust), no el modelo; un verificador marca las
cifras que el modelo no puede justificar con una fuente; las citas apuntan a documentos concretos. Y mide sus propios
aciertos con una regla fija y lo publica aunque el resultado no favorezca a la app.

## Operating Context

- Servicio de usuario de systemd (`./invest-ai install`), en `127.0.0.1:8787`, con entrada en el menú de KDE.
- Lista de seguimiento: una pasada de análisis por día de bolsa, tras el cierre de EE. UU.
- Fuentes de datos: Yahoo, Finnhub, Twelve Data, FRED, SEC y fuentes opcionales con clave; redes sociales (Reddit, X) solo
  como sentimiento y desactivadas sin credenciales.
- Interfaz en español.

## Capabilities and Constraints

- Panel, ficha de activo con análisis por etapas (rápido o estándar), chat con herramientas sobre datos en vivo, favoritos,
  historial, rendimiento de los análisis (learning), OPV con tasa base por tamaño, ajustes de modelos según el hardware.
- RAG local con noticias y folletos (archivado separado de la recuperación).
- Restricción: es análisis automatizado, no asesoramiento financiero.
- Restricción: un solo usuario, sin autenticación; solo loopback.

## Brand Commitments

- Nombre: **invest-ai**.
- Solo afirmaciones que el código demuestra (local, fuentes citadas, verificador de cifras, cifras calculadas en Rust).
- Aviso visible: la información es análisis automatizado, no asesoramiento financiero.
- Ninguna promesa de rentabilidad ni de acierto.

## Evidence on Hand

- Mediciones reales en `docs/notes/`: comparación de modelos (`model-eval.md`), backtest y su resultado honesto: la
  puntuación no mostró ventaja frente al mercado, el riesgo-precio era beta (`backtest.md`), ablación del análisis
  (`ablation.md`), estudio de OPV (`ipo-risk.md`), medición del RAG (`rag.md`).
- No hay testimonios, clientes, prensa ni cifras de rentabilidad: no deben inventarse.
- No hay capturas ni logotipo en el repositorio.

## Product Principles

1. Comprobable antes que convincente: cada conclusión enseña de dónde sale.
2. Privado por construcción: los datos y el modelo no salen de la máquina.
3. Honesto con sus límites: publica lo que no funciona igual que lo que sí.
4. El código calcula, el modelo explica.
