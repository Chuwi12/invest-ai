---
version: 1
slug: "frontend-src-pages-landingpage-tsx"
primary_target: "frontend/src/pages/LandingPage.tsx"
related_targets: []
---

# Landing (ruta `/`)

## Alcance y modo
Persuade. Primera pantalla al abrir la app (`/`); el panel pasa a `/dashboard` detrás de «Entrar al panel».

## Audiencia, trabajo y acción
- Terceros a los que el autor enseña la app (portfolio, demos, contactos profesionales): en segundos deben entender qué es, que cada cifra se puede comprobar y que todo corre en local.
- Acción principal: «Entrar al panel».
- Prueba: datos REALES de la instancia (último análisis guardado, cotizaciones en vivo de la lista, estado del modelo y de la pasada). Sin datos de ejemplo: con la instancia vacía, estado vacío honesto que invita a entrar y analizar.
- Restricciones: solo hechos verificables, sin promesas de rentabilidad, aviso «no es asesoramiento financiero» visible.

## Dirección elegida
Extracto suizo, modernizado a petición del usuario: una cinta de activos que avanza de izquierda a derecha sin cesar.

## Pendiente de decidir
- Si el panel hereda después esta identidad (fuera de alcance ahora).

## Direction contract

THESIS: El análisis como extracto de banca privada suiza: cada cifra en su columna con su referencia y nada sale de la máquina. Rechaza la landing fintech (titular + captura + tres tarjetas) y el editorial crema.

OWN-WORLD: Papel #ffffff, tinta #000000, rojo señal #e2001a solo en campos grandes (bloque de acción, veredicto), rejilla #d9d9d6, subida #127a3e, bajada en rojo solo en cifras. Una sola familia: Archivo variable (anchura y peso), cifras tabulares. Rejilla de 12 columnas, bandera a la izquierda, filetes de 3 px y 1 px, esquinas rectas, sin sombras, sin degradados, sin iconos decorativos.

STORY: El visitante ve un análisis real, comprueba de dónde sale cada cifra, ve que el modelo es local y que la app publica sus aciertos y fallos, y entra al panel.

FIRST VIEWPORT: Arriba, invest-ai y el estado local (modelo cargado, última pasada). Debajo, a todo el ancho, la cinta de activos reales de la lista con precio y variación, avanzando de izquierda a derecha sin parar, con botón de pausa. Columnas 1-7: el activo y el veredicto a cuerpo monumental (veredicto en rojo) y su resumen. Columnas 9-12: cuatro cifras estructuradas con referencia en superíndice. Al pie: índice de fuentes con su antigüedad y el bloque rojo «Entrar al panel».

FORM: Estilo tipográfico internacional con extractos de banca privada; candidato 4 de 7. Interacción firma: al señalar una cifra se resalta su fuente en el índice y viceversa; la cinta es el único movimiento continuo. seed:3e2306bfef533676

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
