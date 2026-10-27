---
name: invest-ai
description: "Extracto suizo: análisis de inversión con IA local, cada cifra en su columna y con su fuente (hoy solo la landing)."
colors:
  paper: "#ffffff"
  ink: "#000000"
  ink-2: "#4a4a4a"
  faint: "#737373"
  rule: "#d9d9d6"
  red: "#e2001a"
  up: "#127a3e"
typography:
  wordmark:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.75rem"
    fontWeight: 800
    lineHeight: 1
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 88"
  display:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "min(28cqi, 14rem)"
    fontWeight: 800
    lineHeight: 0.84
    letterSpacing: "-0.035em"
    fontVariation: "'wdth' 74"
  display-verdict:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "min(17cqi, 8.4rem)"
    fontWeight: 800
    lineHeight: 0.84
    letterSpacing: "-0.035em"
    fontVariation: "'wdth' 74"
  headline:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "clamp(2rem, 3.6vw, 3rem)"
    fontWeight: 800
    lineHeight: 1
    letterSpacing: "-0.025em"
    fontVariation: "'wdth' 92"
  figure:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "min(3.5rem, 13cqi)"
    fontWeight: 600
    lineHeight: 1
    letterSpacing: "-0.02em"
    fontFeature: "'tnum' 1"
  cta:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "clamp(1.75rem, 2.6vw, 2.25rem)"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.02em"
  cta-large:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "clamp(2.5rem, 6vw, 4.5rem)"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "-0.03em"
  title:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.5rem"
    fontWeight: 700
    lineHeight: 1.15
    letterSpacing: "-0.01em"
  title-small:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.25rem"
    fontWeight: 700
    lineHeight: 1.2
    letterSpacing: "-0.01em"
  lead:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.25rem"
    fontWeight: 400
    lineHeight: 1.4
  body:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.0625rem"
    fontWeight: 400
    lineHeight: 1.5
    fontFeature: "'tnum' 1"
  ui:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "0.9375rem"
    fontWeight: 400
    lineHeight: 1.35
  ui-strong:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "0.9375rem"
    fontWeight: 600
    lineHeight: 1.4
  label:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 600
    lineHeight: 1.5
  note:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 400
    lineHeight: 1.5
rounded:
  none: "0px"
spacing:
  page-inline: "clamp(1rem, 3vw, 2rem)"
  gutter: "1.5rem"
  row: "2rem"
  block-start: "1.5rem"
  block-end: "3rem"
components:
  cta-field:
    backgroundColor: "{colors.red}"
    textColor: "{colors.paper}"
    typography: "{typography.cta}"
    rounded: "{rounded.none}"
    padding: "1.25rem"
    height: "9rem"
  cta-field-hover:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
  cta-field-large:
    backgroundColor: "{colors.red}"
    textColor: "{colors.paper}"
    typography: "{typography.cta-large}"
    rounded: "{rounded.none}"
    padding: "1.25rem"
    height: "14rem"
  button-outline:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.ui-strong}"
    rounded: "{rounded.none}"
    padding: "0 0.9rem"
    height: "2.4rem"
  button-outline-hover:
    backgroundColor: "{colors.rule}"
  button-outline-pressed:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
  button-outline-pressed-hover:
    backgroundColor: "{colors.ink-2}"
    textColor: "{colors.paper}"
  tape-cell:
    typography: "{typography.body}"
    padding: "0.55rem 1rem 0.65rem"
    width: "13rem"
    height: "4.25rem"
  figure-row:
    typography: "{typography.figure}"
    padding: "0.6rem 0.75rem 0.65rem"
  figure-row-active:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
  source-row:
    typography: "{typography.ui}"
    padding: "0.25rem 0.5rem 0.3rem"
  source-row-active:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
  status-square:
    backgroundColor: "{colors.up}"
    rounded: "{rounded.none}"
    size: "0.5rem"
  status-square-off:
    backgroundColor: "transparent"
    rounded: "{rounded.none}"
    size: "0.5rem"
---

# Design System: invest-ai

## Overview

**Creative North Star: "El extracto suizo"**

El análisis se presenta como un extracto de banca privada suiza leído con los ojos del estilo tipográfico internacional: papel blanco, tinta negra, cada cifra en su columna con su referencia, y nada que adorne. La autoridad no sale de efectos sino de la disciplina: una rejilla de 12 columnas, una sola familia tipográfica que cambia de anchura y de peso, filetes de 3 px entre bloques y de 1 px entre filas. El rojo señal aparece poco y en grande, como en un sello: el veredicto y el bloque de acción.

La densidad es la de un documento financiero, no la de una landing: la primera pantalla ocupa todo el alto y ya trae datos reales (estado del modelo local, cinta de cotizaciones, el último análisis con sus cuatro cifras y su índice de fuentes). Lo monumental (símbolo y veredicto) convive con texto de 13 a 17 px sin escalones intermedios decorativos. El único movimiento continuo es la cinta; todo lo demás es estático y los cambios de estado son instantáneos, sin transiciones.

**Alcance.** Este mundo rige hoy solo la landing (ruta `/`, `frontend/src/pages/LandingPage.tsx` y `frontend/src/styles/landing.css`, todo bajo `.lp` y clases `lp-*`). Las superficies de la app (panel, ficha de activo, chat y demás, en `frontend/src/styles/app.css`) siguen con su identidad anterior y aún no usan este sistema; si lo heredan es una decisión pendiente del usuario. Este documento no describe ese sistema anterior. El mundo es solo claro: es papel, y no tiene tema oscuro aunque la app lo tenga.

**Key Characteristics:**
- Papel blanco y tinta negra; rojo señal solo en campos grandes y en cifras de bajada.
- Una sola familia, Archivo variable, con la jerarquía hecha de anchura (74 % a 100 %) y peso (400 a 800); cifras tabulares en todo.
- Rejilla de 12 columnas con bandera a la izquierda; filetes de 3 px entre bloques y de 1 px entre filas.
- Esquinas rectas, sin sombras, sin degradados, sin iconos decorativos.
- Resaltar es invertir: fondo tinta, texto papel.
- La cinta de cotizaciones, de izquierda a derecha y con pausa, es el único movimiento continuo.

## Colors

Monocromo de papel y tinta con dos colores de dato: un rojo señal con presencia de sello y un verde que solo dice «sube».

### Primary
- **Rojo señal** (#e2001a): campos grandes y nada más. Rellena el bloque de acción «Entrar al panel» (texto blanco, 4,9:1) y tiñe la palabra monumental del veredicto. Como color de dato, marca las variaciones negativas en la cinta.

### Secondary
- **Verde subida** (#127a3e): exclusivamente semántico. Variaciones positivas de la cinta (5,4:1 sobre papel) y el cuadrado de estado lleno («sí»: modelo cargado, ticks en vivo). Nunca decora.

### Neutral
- **Papel** (#ffffff): el fondo de todo el mundo, también la página y la barra de desplazamiento; texto sobre rojo y sobre tinta.
- **Tinta** (#000000): texto principal, filetes de bloque de 3 px, borde del botón, anillo de foco y fondo de toda inversión (resalte, botón pulsado, selección, enlace de salto, bloque de acción al señalarlo).
- **Tinta segunda** (#4a4a4a): texto secundario (lema, etiquetas de estado, procedencias, metadatos, cabeceras de tabla, variación plana); 8,9:1 sobre papel.
- **Tinta tenue** (#737373): fuentes del índice con más de dos días; atenuadas pero legibles (4,7:1).
- **Gris filete** (#d9d9d6): filetes de 1 px entre filas, separadores de celdas de la cinta y fondo del botón de contorno al señalarlo. No se usa para texto.

### Named Rules
**La regla del campo rojo.** El rojo señal ocupa superficies grandes (el bloque de acción, la palabra del veredicto) o tiñe cifras de bajada. No es color de texto corriente, de filetes, de bordes ni de adornos.

**La regla de la tinta invertida.** Resaltar es invertir: fondo tinta, texto papel. Así se marcan la cifra y la fuente activas, la fuente enlazada (`:target`), el botón pulsado y la selección de texto. No existe un color de resalte propio; dentro de una zona invertida, la tinta segunda pasa a papel y la tenue a gris filete.

**La regla del verde que dice sí.** El verde solo significa «sube» o «sí», y siempre acompañado de texto o de cifra.

## Typography

**Display Font:** Archivo Variable (con Helvetica Neue, Helvetica, Arial, sans-serif)
**Body Font:** Archivo Variable (la misma familia)

**Character:** Una grotesca de anchura variable hace todo el trabajo: estrechada al 74 % y en negra 800 da el cuerpo monumental del símbolo y del veredicto; a anchura normal y peso 400 da el texto de lectura. Cifras tabulares en toda la página (`font-variant-numeric: tabular-nums` en la raíz), para que las columnas de números cuadren.

### Hierarchy
- **Wordmark** (800, 1.75rem, 1, anchura 88 %, -0.03em): el nombre «invest-ai» en la cabecera. Es el único logotipo; no hay imagen de marca.
- **Display** (800, min(28cqi, 14rem), 0.84, anchura 74 %, -0.035em): el símbolo del activo, en tinta. El tamaño se mide contra su columna (contenedor) y contra su número de letras (`--lp-len`, ~0,6 em por letra) para no desbordar nunca.
- **Display verdict** (800, min(17cqi, 8.4rem), 0.84, anchura 74 %): la palabra del veredicto, en rojo señal, ~60 % del símbolo. Sin `cqi`, ambos caen a clamp(3.5rem, 11vw, 6rem). El estado vacío usa el registro monumental reducido y en tinta (clamp(2.25rem, 5vw, 4rem), anchura 86 %, interlínea 0.95, equilibrado).
- **Headline** (800, clamp(2rem, 3.6vw, 3rem), 1, anchura 92 %, -0.025em): titulares de sección bajo el pliegue, con `text-wrap: balance`.
- **Figure** (600, hasta 3.5rem, 1, -0.02em, tabulares): las cuatro cifras del análisis. Se encogen con la columna (13cqi) y con la longitud del importe para no pasar del ~60 % de su columna. La unidad va en 15 px 400 tinta segunda; la referencia, en superíndice de 13 px 700 subrayado.
- **CTA** (700, clamp(1.75rem, 2.6vw, 2.25rem), 1, -0.02em) y **CTA large** (700, clamp(2.5rem, 6vw, 4.5rem), -0.03em): el texto del bloque de acción, arriba a la izquierda del campo rojo.
- **Title** (700, 1.5rem, 1.15, -0.01em): los pasos de «Cómo llega a cada cifra».
- **Title small** (700, 1.25rem, 1.2, -0.01em): títulos de bloque menores («Fuentes de este análisis», «Backtest de la puntuación»).
- **Lead** (400, 1.25rem, 1.4): el resumen del análisis, máximo 40rem; baja a 17 px en móvil.
- **Body** (400, 1.0625rem, 1.5): texto de lectura, máximo 38rem en pasos y notas.
- **UI** (400, 0.9375rem, 1.35) y **UI strong** (600, 0.9375rem, 1.4): lema, valores de estado, índice de fuentes, tabla, botones y enlaces de acción.
- **Label** (600, 0.8125rem) y **Note** (400, 0.8125rem): etiquetas de dato (términos de estado, cabeceras de tabla) y metadatos (procedencia de una cifra, proveedor y antigüedad de una fuente).

Escala de texto: 13 / 15 / 17 / 20 / 24 px, y por encima los registros fluidos de titular, CTA, cifra y monumental.

### Named Rules
**La regla de una sola familia.** Todo es Archivo variable. La jerarquía sale de la anchura y del peso, nunca de una segunda familia.

**La regla de la columna de cifras.** Toda cifra va en tabulares y alineada a la derecha de su columna. Lo monumental y las cifras se dimensionan contra su columna y su número de caracteres: ningún símbolo, veredicto ni importe desborda.

## Layout

Rejilla de 12 columnas (`minmax(0, 1fr)`) con calle de 1.5rem y separación de filas de 2rem. El contenido se centra en un máximo de 90rem con margen lateral fluido (clamp(1rem, 3vw, 2rem)), pero los bloques no son contenedores: el relleno se calcula para centrar el contenido, de modo que el filete de 3 px de cada bloque corre de borde a borde de la ventana.

Repartos de columnas: principal 1–7 y lateral 9–12 (la columna 8 queda vacía como aire entre el monumental y las cifras); ancho 1–8 junto a la acción en 9–12; etiqueta 1–5 y cuerpo 6–12 bajo el pliegue. Bandera a la izquierda: nada se centra; las cifras se alinean a la derecha de su columna.

Cada bloque empieza con su filete y un relleno superior de 1.5rem (1rem en los bloques de la primera pantalla, 2rem bajo el pliegue) y cierra con 3rem. La primera pantalla ocupa 100dvh: cabecera con estado, cinta a todo el ancho, análisis que crece para llenar el alto, y al pie el índice de fuentes junto al bloque de acción. Bajo el pliegue, el titular de «Cómo llega a cada cifra» se queda fijo (sticky a 1.5rem) mientras se leen sus pasos.

Responsive: hasta 72rem el índice de fuentes pasa de dos columnas (que se leen de arriba abajo) a una y el bloque de acción sube al inicio de su fila; hasta 56rem la rejilla es de una columna, la cabecera se apila, el estado se compacta en una línea por dato, el bloque de acción del pie baja a 6rem de alto mínimo y el de cierre a 10rem. En el DOM la acción va antes que el índice, para que en móvil el orden visual y el del foco coincidan.

## Elevation & Depth

Plano por completo: ni sombras, ni degradados, ni desenfoques, ni capas translúcidas. La estructura se lee por filetes (3 px de tinta entre bloques, 1 px gris entre filas y celdas) y por inversión de tinta y papel. El único apilamiento es funcional: los controles de la cinta quedan por encima de la pista que pasa por debajo.

### Named Rules
**La regla del papel plano.** Nada flota. Si algo tiene que destacar, se invierte o se agranda; no se eleva.

## Shapes

Ángulo recto en todo: radio 0 en botones, campos, celdas y también en el anillo de foco. La forma la ponen los filetes horizontales, que separan sin encerrar (no hay tarjetas con borde a cuatro lados, salvo el botón de contorno). El único signo gráfico es un cuadrado de 0.5rem con borde de 2 px: lleno en verde para «sí», hueco con borde de tinta segunda para «no», siempre junto a su texto. Los enlaces se marcan con subrayado (desplazado 0.2em; 2 px al señalar un símbolo de la cinta).

### Named Rules
**La regla del ángulo recto.** Ninguna esquina se redondea, ni siquiera la del foco.

## Components

### Buttons
Sobrios y rectos: un campo rojo para la acción principal, un contorno de tinta para controlar, un subrayado para reintentar.
- **Shape:** esquinas rectas (0px).
- **Bloque de acción (primario):** campo rojo señal con texto blanco en 700, alineado arriba a la izquierda, 1.25rem de relleno, 9rem de alto en el pie y 14rem (10rem en móvil) en el cierre, con el texto en el registro CTA large. Es un enlace a `/dashboard`, no un botón con forma de píldora.
- **Hover / Focus:** al señalarlo el campo pasa a tinta, sin transición. El foco es un anillo blanco de 3 px metido hacia dentro (desplazamiento -0.6rem) para verse sobre el rojo.
- **Contorno (secundario):** «Pausar cinta». Borde de 1 px tinta, fondo transparente, 600 a 15 px, 2.4rem de alto. Al señalarlo, fondo gris filete; pulsado (`aria-pressed`), fondo tinta y texto papel; pulsado y señalado, fondo tinta segunda.
- **Enlace de acción (terciario):** «Reintentar». Sin caja, 600 a 15 px, subrayado.

### Cinta de cotizaciones (firma)
- **Estructura:** a todo el ancho bajo la cabecera, 4.25rem de alto mínimo. A la izquierda, alineados con el margen de la página, el botón de pausa y el indicador «En vivo» con su cuadrado; a la derecha, celdas de ancho fijo de 13rem separadas por filetes grises de 1 px: símbolo en 700, variación en 600 a 15 px a la derecha (verde, rojo o tinta segunda), precio en 600 a 1.25rem.
- **Movimiento:** avanza de izquierda a derecha, lineal y sin fin, a 3.5 rem/s, con dos mitades idénticas para que el bucle no tenga costura. Solo anima `transform`. Se detiene al señalarla, con el botón de pausa y con el foco dentro (entonces pasa a desplazamiento horizontal y la celda enfocada se ve entera). Con «reducir movimiento» no se mueve, se recorre con scroll y el botón de pausa desaparece.
- **Estados:** sin precio, «—» o «sin precio» en tinta segunda; si falla la carga, aviso con filete superior y enlace «Reintentar».

### Columna de cifras (firma)
- **Estructura:** lista de definición de cuatro filas separadas por filetes de 1 px. A la izquierda, la etiqueta en 600 a 15 px y debajo su procedencia en 13 px tinta segunda; a la derecha, la cifra en el registro Figure, con unidad y referencia en superíndice.
- **Interacción:** señalar o enfocar una cifra con fuente invierte su fila y la fila de su fuente en el índice; señalar una fuente invierte la cifra que la cita. Seguir la referencia lleva a la fuente, que queda invertida por `:target`.

### Índice de fuentes
- **Style:** lista numerada en dos columnas que se leen de arriba abajo (una por debajo de 72rem). Número en 700, título en 15 px truncado con puntos suspensivos en escritorio, y debajo proveedor y antigüedad en 13 px tinta segunda. Filete superior de 1 px por fila.
- **State:** una fuente con más de dos días pasa entera a tinta tenue; activa o enlazada, se invierte.

### Estado local
- **Style:** en la cabecera, pares término y valor: término en 13 px 600 tinta segunda, valor en 15 px con el cuadrado de estado delante. En móvil, término y valor en la misma línea.

### Tablas
- **Style:** a ancho completo, filas separadas por filetes de 1 px (con filete también al final), cabeceras de columna en 13 px 600 tinta segunda sin filete, cabeceras de fila en 700. Sin bandas de color ni bordes verticales.

### Lista de pasos
- **Style:** filas separadas por filetes de 1 px, título de paso en el registro Title y texto de lectura debajo; la prueba en vivo (notas de calidad reales) se añade bajo su paso tras un filete de 1 px.

## Do's and Don'ts

### Do:
- **Do** empezar cada bloque con un filete de tinta de 3 px de borde a borde y separar cada fila con un filete gris de 1 px (#d9d9d6).
- **Do** poner cada cifra en tabulares, alineada a la derecha de su columna, con su procedencia bajo la etiqueta y su referencia en superíndice cuando tenga fuente.
- **Do** resaltar invirtiendo tinta y papel, nunca con un color nuevo.
- **Do** acompañar siempre el cuadrado de estado con texto: lleno en verde para «sí», hueco para «no».
- **Do** dar a todo lo enfocable un anillo de tinta de 3 px separado 3 px y con esquinas rectas; sobre rojo, blanco y hacia dentro.
- **Do** dimensionar lo monumental y las cifras contra su columna y su número de caracteres, para que nada desborde.
- **Do** detener la cinta con «reducir movimiento», al señalarla, al enfocarla y con su botón de pausa.

### Don't:
- **Don't** usar el rojo señal para texto corriente, filetes, bordes o adornos: solo campos grandes y cifras de bajada.
- **Don't** añadir sombras, degradados, desenfoques ni esquinas redondeadas.
- **Don't** introducir una segunda familia tipográfica, tampoco las de las superficies de la app (Geist, Newsreader), ni su acento azul.
- **Don't** añadir iconos decorativos: el único signo gráfico es el cuadrado de estado, y siempre lleva texto.
- **Don't** usar el estilo de etiqueta (13 px, 600) como antetítulo sobre un titular: solo nombra un dato (término de estado, cabecera de tabla).
- **Don't** centrar titulares ni bloques de texto.
- **Don't** añadir un segundo movimiento continuo: la cinta es el único.
- **Don't** dar a este mundo un tema oscuro: es papel.
