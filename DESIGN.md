---
name: invest-ai
description: "Extracto suizo: análisis de inversión con IA local, cada cifra en su columna y con su fuente. Un solo mundo para la landing y el panel, en claro y en oscuro."
colors:
  # Tema claro (por defecto). Mismos nombres que frontend/src/styles/tokens.css.
  bg: "#ffffff"
  surface: "#ffffff"
  surface-2: "#f2f2f0"
  hover: "color-mix(in srgb, #f2f2f0 70%, #ffffff)"
  ink: "#000000"
  ink-2: "#4a4a4a"
  muted: "#6b6b6b"
  line: "#d9d9d6"
  line-strong: "#000000"
  accent: "#000000"
  accent-ink: "#ffffff"
  accent-soft: "#e6e6e3"
  focus: "#000000"
  signal: "#e2001a"
  signal-ink: "#ffffff"
  up: "#127a3e"
  up-soft: "#e3f1e8"
  down: "#e2001a"
  down-soft: "#fff3f4"
  warn: "#8a5a00"
  warn-soft: "#f5e8c8"
  # Tema oscuro (prefers-color-scheme: dark o data-theme="dark"). signal y signal-ink no cambian.
  bg-dark: "#000000"
  surface-dark: "#000000"
  surface-2-dark: "#161616"
  hover-dark: "#262626"
  ink-dark: "#ffffff"
  ink-2-dark: "#bdbdbd"
  muted-dark: "#8c8c8c"
  line-dark: "#333333"
  line-strong-dark: "#ffffff"
  accent-dark: "#ffffff"
  accent-ink-dark: "#000000"
  accent-soft-dark: "#262626"
  focus-dark: "#ffffff"
  up-dark: "#3fbf72"
  up-soft-dark: "#0f2a1b"
  down-dark: "#ff4d5a"
  down-soft-dark: "#3a0d12"
  warn-dark: "#e6b450"
  warn-soft-dark: "#3a2f12"
typography:
  wordmark:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.75rem"
    fontWeight: 800
    lineHeight: 1
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 88"
  wordmark-rail:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.25rem"
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
  page-title:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1.75rem"
    fontWeight: 800
    lineHeight: 1.1
    letterSpacing: "-0.025em"
    fontVariation: "'wdth' 92"
  verdict:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "2.6rem"
    fontWeight: 800
    lineHeight: 1
    letterSpacing: "-0.035em"
    fontVariation: "'wdth' 74"
  figure:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "min(3.5rem, 13cqi)"
    fontWeight: 600
    lineHeight: 1
    letterSpacing: "-0.02em"
    fontFeature: "'tnum' 1"
  price:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "2.6rem"
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
  section-title:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "1rem"
    fontWeight: 700
    lineHeight: 1.5
    letterSpacing: "-0.01em"
    fontVariation: "'wdth' 92"
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
    lineHeight: 1.5
    fontFeature: "'tnum' 1"
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
  meta:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: 1.5
  meta-strong:
    fontFamily: "'Archivo Variable', 'Helvetica Neue', Helvetica, Arial, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 600
    lineHeight: 1.5
rounded:
  none: "0px"
spacing:
  page-inline: "clamp(1rem, 3vw, 2rem)"
  gutter: "1.5rem"
  row: "2rem"
  block-start: "1.5rem"
  block-end: "3rem"
  panel-pad: "1.25rem"
  panel-stack: "1.5rem"
  panel-grid: "1.25rem"
components:
  cta-field:
    backgroundColor: "{colors.signal}"
    textColor: "{colors.signal-ink}"
    typography: "{typography.cta}"
    rounded: "{rounded.none}"
    padding: "1.25rem"
    height: "9rem"
  cta-field-hover:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.bg}"
  cta-field-large:
    backgroundColor: "{colors.signal}"
    textColor: "{colors.signal-ink}"
    typography: "{typography.cta-large}"
    rounded: "{rounded.none}"
    padding: "1.25rem"
    height: "14rem"
  brand-field:
    backgroundColor: "{colors.signal}"
    textColor: "{colors.signal-ink}"
    typography: "{typography.wordmark-rail}"
    rounded: "{rounded.none}"
    padding: "0.7rem 0.75rem"
  nav-item:
    textColor: "{colors.ink-2}"
    typography: "{typography.ui-strong}"
    rounded: "{rounded.none}"
    padding: "0.5rem 0.6rem"
  nav-item-hover:
    backgroundColor: "{colors.hover}"
    textColor: "{colors.ink}"
  nav-item-active:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.ui-strong}"
    rounded: "{rounded.none}"
    padding: "0 0.85rem"
    height: "2.1rem"
  button-primary-hover:
    backgroundColor: "{colors.ink-2}"
    textColor: "{colors.accent-ink}"
  button-outline:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.ui-strong}"
    rounded: "{rounded.none}"
    padding: "0 0.85rem"
    height: "2.1rem"
  button-outline-hover:
    backgroundColor: "{colors.line}"
  button-outline-pressed:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.bg}"
  button-outline-pressed-hover:
    backgroundColor: "{colors.ink-2}"
    textColor: "{colors.bg}"
  button-outline-large:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.ui-strong}"
    rounded: "{rounded.none}"
    padding: "0 0.9rem"
    height: "2.4rem"
  button-small:
    typography: "{typography.label}"
    rounded: "{rounded.none}"
    padding: "0 0.6rem"
    height: "1.75rem"
  button-ghost:
    backgroundColor: "transparent"
    rounded: "{rounded.none}"
  chip:
    backgroundColor: "{colors.accent-soft}"
    textColor: "{colors.ink}"
    typography: "{typography.meta-strong}"
    rounded: "{rounded.none}"
    padding: "0.1rem 0.5rem"
  chip-muted:
    backgroundColor: "{colors.accent-soft}"
    textColor: "{colors.ink-2}"
  chip-up:
    backgroundColor: "{colors.up-soft}"
    textColor: "{colors.up}"
  chip-down:
    backgroundColor: "{colors.down-soft}"
    textColor: "{colors.down}"
  chip-warn:
    backgroundColor: "{colors.warn-soft}"
    textColor: "{colors.warn}"
  chip-action:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.meta-strong}"
    rounded: "{rounded.none}"
    padding: "0.1rem 0.5rem"
  chip-action-hover:
    backgroundColor: "{colors.line}"
  chip-action-pressed:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
  segment:
    backgroundColor: "transparent"
    textColor: "{colors.ink-2}"
    typography: "{typography.label}"
    padding: "0.25rem 0.6rem"
  segment-hover:
    backgroundColor: "{colors.line}"
  segment-selected:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
  input:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.ui}"
    rounded: "{rounded.none}"
    padding: "0.45rem 0.65rem"
  search-trigger:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.muted}"
    typography: "{typography.ui}"
    rounded: "{rounded.none}"
    padding: "0 0.7rem"
    height: "2.25rem"
  table-row:
    typography: "{typography.ui}"
    padding: "0.5rem 0.6rem"
  table-row-hover:
    backgroundColor: "{colors.hover}"
  notice:
    backgroundColor: "{colors.surface-2}"
    textColor: "{colors.ink}"
    padding: "0.6rem 0.8rem"
  notice-warn:
    backgroundColor: "{colors.warn-soft}"
    textColor: "{colors.warn}"
  notice-error:
    backgroundColor: "{colors.warn-soft}"
    textColor: "{colors.ink}"
  mock-banner:
    backgroundColor: "{colors.warn-soft}"
    textColor: "{colors.warn}"
    typography: "{typography.label}"
    padding: "0.3rem 1rem"
  verdict-up:
    textColor: "{colors.up}"
    typography: "{typography.verdict}"
  verdict-down:
    textColor: "{colors.down}"
    typography: "{typography.verdict}"
  verdict-hold:
    textColor: "{colors.ink}"
    typography: "{typography.verdict}"
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
    textColor: "{colors.bg}"
  source-row:
    typography: "{typography.ui}"
    padding: "0.25rem 0.5rem 0.3rem"
  source-row-active:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.bg}"
  status-square:
    backgroundColor: "{colors.up}"
    rounded: "{rounded.none}"
    size: "0.5rem"
  status-square-off:
    backgroundColor: "transparent"
    rounded: "{rounded.none}"
    size: "0.5rem"
  status-dot:
    backgroundColor: "{colors.muted}"
    rounded: "{rounded.none}"
    size: "0.55rem"
  status-dot-ok:
    backgroundColor: "{colors.up}"
  status-dot-bad:
    backgroundColor: "{colors.warn}"
---

# Design System: invest-ai

## Overview

**Creative North Star: "El extracto suizo"**

El análisis se presenta como un extracto de banca privada suiza leído con los ojos del estilo tipográfico internacional: papel y tinta, cada cifra en su columna con su referencia, y nada que adorne. La autoridad no sale de efectos sino de la disciplina: una sola familia tipográfica que cambia de anchura y de peso, filetes de 3 px que abren bloques y de 1 px entre filas, esquinas rectas y ninguna sombra. Toda acción, selección y foco va en tinta; el rojo señal aparece en dos sitios y en grande, como un sello: la marca del raíl y el bloque de acción de la landing.

Un solo mundo rige toda la app con el mismo contrato de tokens (`frontend/src/styles/tokens.css`; la landing los consume a través de alias `--lp-*`). La landing (ruta `/`, modo Persuade) es monumental: primera pantalla a todo el alto con datos reales, símbolo y veredicto a cuerpo de cartel, rejilla de 12 columnas. El panel (todas las rutas dentro del Shell: panel, ficha e informe, chat, rendimiento, OPV, modelos, favoritos e historial, modo Operate) es denso y escaneable: raíl, tablas, filetes y texto de 12 a 17 px; allí la expresión vive en los detalles, no en el gesto.

El mundo tiene dos temas. En claro es papel blanco y tinta negra; en oscuro, suelo negro y tinta blanca: el mismo extracto invertido, no un tema gris. Manda `prefers-color-scheme` salvo que el usuario fije claro u oscuro con el conmutador (sistema → claro → oscuro), que en el panel es un icono en la barra superior y en la landing un botón de texto «Tema: …»; un script previo fija `data-theme` antes del primer pintado para que no haya destello. Solo se mueve lo que informa (el destello de un precio que cambia, la barra de carga, el cursor del chat) y la cinta de la landing; los cambios de estado son instantáneos, sin transiciones.

**Key Characteristics:**
- Papel y tinta en los dos temas: blanco y negro en claro, negro y blanco en oscuro.
- Acciones, selección y foco en tinta; resaltar es invertir.
- Rojo señal solo en dos campos: la marca del raíl y el bloque de acción de la landing. El rojo de dato significa bajada.
- Todo veredicto lleva color direccional: verde comprar y acumular, rojo reducir y evitar, tinta mantener y esperar.
- Una sola familia, Archivo variable, con la jerarquía hecha de anchura (74 % a 100 %) y peso (400 a 800); cifras tabulares en todo.
- Filetes de 3 px que abren bloques y de 1 px entre filas; esquinas rectas; sin sombras, degradados ni desenfoques.
- La cinta de cotizaciones es el único movimiento continuo que no informa de un estado.

## Colors

Monocromo de papel y tinta en dos temas, con un rojo de sello para la marca y la acción, y tres colores de dato: verde sube, rojo baja, ámbar avisa. Cada token claro tiene su pareja `-dark`; los componentes nombran el rol y el tema elige el valor.

### Primary
- **Tinta de acción** (`accent`, en oscuro `accent-dark`): botón primario, elemento de navegación activo (raíl y barra móvil), opción segmentada seleccionada, pestaña activa (filete inferior de 3 px), chip pulsado, conversación y OPV actuales, opción de la paleta de búsqueda, selección de texto, enlaces del texto del chat, barras de puntuación y gráfico de fiabilidad. Sobre ella, `accent-ink`. El anillo de foco (`focus`) y el filete fuerte (`line-strong`) comparten el valor de la tinta.

### Secondary
- **Rojo señal** (`signal`, igual en los dos temas, con texto `signal-ink`, 4,94:1): solo dos campos grandes. La marca «invest-ai» en el raíl del panel y el bloque de acción «Entrar al panel» de la landing (pie y cierre). No es texto, ni filete, ni borde, ni veredicto, ni error.

### Tertiary
- **Verde subida** (`up` / `up-dark`): sube, sí, comprar y acumular, escenario alcista, sentimiento positivo, estado correcto. 5,41:1 sobre papel y 8,91:1 sobre suelo negro; sobre su relleno `up-soft`, 4,64:1 y 6,51:1.
- **Rojo bajada** (`down` / `down-dark`): baja, reducir y evitar, escenario bajista, sentimiento negativo. En claro comparte el hex con el rojo señal pero no el papel: es color de dato, en texto, barras y velas. En oscuro se aclara (6,46:1). Sobre `down-soft`, 4,56:1 en claro y 5,18:1 en oscuro.
- **Ámbar aviso** (`warn` / `warn-dark`): avisos, errores y fallos, datos obsoletos, modo demostración, barra de riesgo y estado «en espera». Sobre `warn-soft`, 4,87:1 en claro y 6,9:1 en oscuro.

### Neutral
- **Papel / suelo** (`bg`, `surface`): fondo de toda la app, de la barra de desplazamiento y de lo que flota; texto sobre tinta.
- **Relleno** (`surface-2`): mensaje del usuario en el chat, código, aviso neutro, cita de evidencia.
- **Señalado** (`hover`): fondo de filas, mosaicos y elementos de navegación al señalarlos, y fila de totales. En claro es un punto más suave que el relleno para que el rojo de las bajadas siga en 4,57:1; en oscuro es el relleno de chip, porque el relleno casi no se ve sobre negro. Es variable local del panel (`app.css`), no del contrato.
- **Tinta** (`ink`): texto principal, filetes de 3 px, bordes de controles, fondo de toda inversión.
- **Tinta segunda** (`ink-2`): texto secundario, cabeceras de tabla, etiquetas de la landing, chip atenuado (7,09:1 sobre su relleno en claro; 8,06:1 en oscuro) y fondo del botón primario al señalarlo.
- **Tinta tenue** (`muted`): metadatos, procedencias, pistas, texto del buscador, fuentes con más de dos días. 5,33:1 sobre papel y 4,75:1 sobre relleno en claro; 6,25:1 en oscuro.
- **Gris filete** (`line`): filetes de 1 px entre filas y celdas, pistas de barras, rejilla del gráfico, fondo del contorno al señalarlo. Nunca texto.
- **Relleno de chip** (`accent-soft`): chips estáticos y esqueletos de carga.

### Named Rules
**La regla de la tinta que actúa.** Toda acción, selección y foco va en tinta: negra en claro, blanca en oscuro. No hay color de acento: el rojo ya significa «baja».

**La regla de la tinta invertida.** Resaltar es invertir: fondo tinta, texto papel. Dentro de una zona de tinta la selección de texto también se invierte, el foco pasa a `accent-ink` y lo atenuado toma el color del texto.

**La regla de los dos rojos.** El rojo señal es un campo de marca: el raíl y el bloque de acción, siempre con texto blanco. El rojo de dato (`down`) significa bajada y nada más. Ninguno de los dos marca un error ni una acción destructiva: borrar es una acción en tinta.

**La regla del veredicto direccional.** Todo veredicto, del chip de una fila al monumental de la landing, lleva el color de su dirección: verde comprar y acumular, rojo reducir y evitar, tinta mantener y esperar.

**La regla del ámbar.** Avisos, errores y fallos van en `warn` sobre `warn-soft`, nunca en rojo: un error no es una bajada.

## Typography

**Display Font:** Archivo Variable (con Helvetica Neue, Helvetica, Arial, sans-serif)
**Body Font:** Archivo Variable (la misma familia)
**Label/Mono Font:** ui-monospace solo para el código y el registro de herramientas del chat

**Character:** Una grotesca de anchura variable hace todo el trabajo: estrechada al 74 % y en negra 800 da el cuerpo monumental del símbolo y del veredicto; a anchura normal y peso 400 da el texto de lectura y la interfaz. Cifras tabulares en toda la app (`font-variant-numeric: tabular-nums` en la raíz de la landing y en el `body` del panel), para que las columnas de números cuadren.

### Hierarchy
Landing:
- **Wordmark** (800, 1.75rem, anchura 88 %): el nombre en la cabecera. En el raíl del panel, **Wordmark rail** (800, 1.25rem) dentro del campo rojo. No hay imagen de marca.
- **Display** (800, min(28cqi, 14rem), 0.84, anchura 74 %): el símbolo del activo, en tinta, medido contra su columna y su número de letras (~0,6 em por letra).
- **Display verdict** (800, min(17cqi, 8.4rem), anchura 74 %): la palabra del veredicto con su color direccional, ~60 % del símbolo. Sin `cqi`, ambos caen a clamp(3.5rem, 11vw, 6rem). El estado vacío usa el registro monumental reducido y en tinta.
- **Headline** (800, clamp(2rem, 3.6vw, 3rem), anchura 92 %): titulares bajo el pliegue, equilibrados.
- **Figure** (600, hasta 3.5rem, tabulares): las cuatro cifras del análisis, encogidas por columna y por longitud del importe.
- **CTA** y **CTA large** (700): el texto del bloque de acción.
- **Title** (700, 1.5rem) y **Title small** (700, 1.25rem): pasos y títulos de bloque menores.
- **Lead** (400, 1.25rem, máximo 40rem): el resumen del análisis; 17 px en móvil.

Panel:
- **Page title** (800, 1.75rem, 1.1, anchura 92 %): el título de cada página; en la ficha de activo sube a 1.9rem (1.6rem en móvil).
- **Verdict** (800, 2.6rem, anchura 74 %): la palabra del veredicto en el informe, con su color direccional; 2.1rem en móvil.
- **Price** (600, 2.6rem, tabulares): el precio grande de la ficha; 2.1rem en móvil.
- **Section title** (700, 1rem, anchura 92 %): el título que acompaña al filete de 3 px de cada sección, con su pista en Meta a la derecha.

Comunes:
- **Body** (400, 1.0625rem, 1.5): texto de lectura; máximo 38rem en la landing y 52ch en el informe y el chat.
- **UI** (400, 0.9375rem, 1.5) y **UI strong** (600): texto base del panel, botones, navegación, enlaces de acción. La landing aprieta la interlínea a 1.35 en el lema y el índice de fuentes.
- **Label** (600, 0.8125rem) y **Note** (400, 0.8125rem): etiquetas de dato y metadatos de la landing; opciones segmentadas y botones pequeños del panel.
- **Meta** (400, 0.75rem) y **Meta strong** (600, 0.75rem): procedencias, pistas, términos de pares clave-valor; chips y cabeceras de tabla del panel.

Escala: 11 / 12 / 13 / 14 / 15 / 16 / 17 / 20 / 24 / 28 px, y por encima los registros de precio, veredicto, titular, CTA, cifra y monumental.

### Named Rules
**La regla de una sola familia.** Todo es Archivo variable, en la landing y en el panel. La jerarquía sale de la anchura y del peso, nunca de una segunda familia.

**La regla de la columna de cifras.** Toda cifra va en tabulares y alineada a la derecha de su columna. Lo monumental y las cifras se dimensionan contra su columna y su número de caracteres: ningún símbolo, veredicto ni importe desborda.

## Layout

**Landing.** Rejilla de 12 columnas (`minmax(0, 1fr)`) con calle de 1.5rem y separación de filas de 2rem, centrada en un máximo de 90rem con margen lateral fluido; los bloques no son contenedores, de modo que el filete de 3 px de cada bloque corre de borde a borde de la ventana. Repartos: principal 1–7 y lateral 9–12 (la 8 queda de aire), ancho 1–8 junto a la acción en 9–12, etiqueta 1–5 y cuerpo 6–12 bajo el pliegue. La primera pantalla ocupa 100dvh: cabecera con estado y conmutador de tema, cinta a todo el ancho, análisis que crece, y al pie el índice de fuentes junto al bloque de acción. Hasta 72rem el índice pasa a una columna; hasta 56rem la rejilla es de una columna y la cabecera se apila. En el DOM la acción va antes que el índice, para que en móvil coincidan el orden visual y el del foco.

**Panel.** El Shell es una rejilla: raíl fijo de 13rem a la izquierda (marca, navegación y, en modo demostración, el chip «Datos simulados»), barra superior fija de 3.25rem (buscador, menú de estado y conmutador de tema), pie fijo de 1.75rem con el aviso legal, y el contenido con 1.25rem de relleno en una página de hasta 90rem. Las secciones se apilan a 1.5rem; las rejillas de 2 y 3 columnas y la de principal más lateral de 22rem separan a 1.25rem. Cada sección se abre con un filete de 3 px y una fila de título; dentro, las filas se separan con filetes de 1 px. Bandera a la izquierda; las cifras se alinean a la derecha. El modo demostración añade una franja ámbar a todo el ancho bajo la barra superior.

Responsive del panel: hasta 1100px la lateral y el calendario de OPV bajan a una columna; hasta 880px el raíl desaparece y la navegación pasa a una barra inferior fija de 3.5rem con siete iconos y etiqueta corta, el relleno baja a 0.9rem 0.75rem y el chat muestra la lista o el hilo; hasta 760px las rejillas de 2 y 3 columnas pasan a una; hasta 520px el selector de profundidad ocupa todo el ancho.

## Elevation & Depth

Plano por completo, en los dos temas: ni sombras, ni degradados, ni desenfoques. La estructura se lee por filetes (3 px de tinta que abren bloques, 1 px de gris entre filas) y por inversión de tinta y papel. Lo que flota (la paleta de búsqueda, el menú de estado) se separa con un filete superior de 3 px y un filete de tinta de 1 px alrededor; el velo de la paleta es papel al 72 % sin desenfoque. El apilamiento restante es funcional: raíl, barra superior, pie y barra móvil fijos, y los controles de la cinta por encima de su pista.

### Shadow Vocabulary
- **Filete flotante** (`box-shadow: 0 0 0 1px var(--ink)`): el único valor de `--shadow-pop`. Es un filete, no una sombra; solo para paleta y menús emergentes.

### Named Rules
**La regla del papel plano.** Nada flota. Si algo tiene que destacar, se invierte o se agranda; no se eleva.

## Shapes

Ángulo recto en todo (`--radius: 0`): botones, campos, chips, segmentados, celdas, cuadrados de estado y anillo de foco. Los filetes horizontales separan sin encerrar: no hay tarjetas con borde a cuatro lados. El contorno de 1 px a cuatro lados es exclusivo de lo que se pulsa o se escribe (botones, chips pulsables, segmentados, campos, buscador, `kbd`).

Signos gráficos: el cuadrado de estado, nunca un círculo (0.5rem con borde de 2 px en la landing: lleno en verde «sí», hueco «no»; 0.55rem lleno en el panel: verde correcto, ámbar fallo (nunca rojo: un fallo no es una bajada), tenue en espera), siempre junto a su texto. El panel usa iconos lineales funcionales (Lucide, de 0.8 a 1.25rem, en el color del texto): navegación, botones de solo icono con nombre accesible, avisos, y flechas de variación que acompañan al signo y al color. La landing no usa iconos. Los enlaces se marcan con subrayado.

### Named Rules
**La regla del ángulo recto.** Ninguna esquina se redondea, ni siquiera la del foco.

## Components

### Buttons
Sobrios y rectos, en tinta: un relleno para la acción principal, un contorno para lo demás.
- **Shape:** esquinas rectas (0px); 2.1rem de alto (1.75rem el pequeño, 2.4rem en la landing).
- **Primario:** relleno de tinta con texto `accent-ink` en 600; al señalarlo, tinta segunda. Deshabilitado, al 50 % de opacidad.
- **Contorno:** borde de 1 px de tinta, fondo de papel, texto en tinta; al señalarlo, gris filete; pulsado (`aria-pressed`), tinta con texto papel. En la landing es «Pausar cinta» y «Tema: …».
- **Fantasma:** sin borde ni fondo, hereda el color: sobre tinta, papel. Es el de los botones de solo icono (conmutador de tema, fijar y borrar conversación). Borrar es tinta, no rojo.
- **Bloque de acción (landing):** campo de rojo señal con texto blanco en 700, alineado arriba a la izquierda, 9rem de alto en el pie y 14rem (10rem en móvil) al cierre. Al señalarlo se invierte a tinta, sin transición; el foco es un anillo blanco de 3 px metido hacia dentro.
- **Focus:** anillo de 3 px en `focus`, recto, separado 2 px en el panel y 3 px en la landing; hacia dentro (-3 px) en segmentados, pestañas y celdas de la cinta; en `accent-ink` sobre tinta.

### Chips
- **Estáticos:** rellenos, sin filete, 600 a 12 px: `accent-soft` con tinta (clase, mercado, contexto), atenuado en `ink-2`, y en `up-soft`, `down-soft` o `warn-soft` con su color para veredictos y estados.
- **Pulsables** (enlace o botón): papel con filete de 1 px de tinta; al señalarlos, gris filete; activos, rellenos de tinta con texto `accent-ink`. El filete es la señal de que se pulsan.

### Segmentados y pestañas
- **Segmentado:** contorno de tinta que recorta, opciones en 600 a 13 px separadas por filetes de 1 px; seleccionada, invertida; foco hacia dentro y en `accent-ink` sobre la opción invertida.
- **Pestañas:** sin caja, texto tenue en 600; la activa pasa a tinta con filete inferior de 3 px de tinta, sobre un filete de 1 px que recorre la fila.

### Inputs / Fields
- **Style:** papel, contorno de 1 px de tinta, recto, 0.45rem 0.65rem de relleno. El buscador de la barra superior es un campo falso de 2.25rem con texto tenue y atajo `kbd` con su propio contorno.
- **Focus:** el anillo común; en la paleta de búsqueda el filete inferior pasa de 1 a 3 px de `focus`, sin anillo.

### Navigation
- **Raíl:** marca en campo de rojo señal arriba; enlaces en 600 con icono, tinta segunda; al señalarlos, fondo `hover` y tinta; el actual, invertido en tinta.
- **Barra móvil:** siete columnas con icono y etiqueta de 11 px en tenue; la actual, invertida.
- **Barra superior:** papel con filete inferior de 1 px; buscador, menú de estado (contorno con cuadrado de estado) y conmutador de tema.

### Tablas
- **Style:** a ancho completo; cabeceras de columna en 600 a 12 px tinta segunda sobre un filete de tinta; filas separadas por filetes de 1 px; cifras a la derecha; al señalar una fila, fondo `hover`; fila de totales en `hover` y 700. Sin bandas ni bordes verticales. En la landing, cabeceras sin filete y filete también al final.

### Avisos
- **Neutro:** relleno `surface-2`, 14 px, icono a la izquierda.
- **Aviso:** ámbar sobre `warn-soft`.
- **Error:** `warn-soft` con filete superior de 3 px de ámbar e icono ámbar; el texto queda en tinta.
- **Modo demostración:** franja ámbar de 13 px en 600 bajo la barra superior y chip ámbar en el raíl.

### Veredicto (firma)
- **Panel:** bloque abierto por filete de 3 px: palabra en el registro Verdict con su color direccional y, a su lado, una escala de seis tramos (pista gris filete, tramo activo en el color del veredicto o en tinta) con sus etiquetas en 11 px.
- **Landing:** la palabra monumental bajo el símbolo, con el mismo color direccional.
- **Chips de veredicto:** relleno suave y texto del color de la dirección; mantener y esperar en el chip neutro.

### Cinta de cotizaciones (firma de la landing)
- **Estructura:** a todo el ancho bajo la cabecera, 4.25rem de alto mínimo; a la izquierda, el botón de pausa y «En vivo» con su cuadrado; a la derecha, celdas de 13rem separadas por filetes de 1 px: símbolo en 700, variación en 600 (verde, rojo o tinta segunda), precio en 600 a 1.25rem.
- **Movimiento:** de izquierda a derecha, lineal y sin fin, a 3.5 rem/s, con dos mitades idénticas; solo anima `transform`. Se detiene al señalarla, con su botón y con el foco dentro (entonces pasa a desplazamiento horizontal). Con «reducir movimiento» no se mueve y el botón desaparece.

### Columna de cifras e índice de fuentes (firma de la landing)
- **Cifras:** cuatro filas separadas por filetes de 1 px: etiqueta en 600 y su procedencia debajo, cifra en el registro Figure con unidad y referencia en superíndice.
- **Índice:** lista numerada en dos columnas que se leen de arriba abajo; una fuente con más de dos días pasa a tinta tenue.
- **Interacción:** señalar una cifra invierte su fila y la de su fuente, y viceversa; la fuente enlazada queda invertida por `:target`.

### Precio en vivo y gráficos (panel)
- **Destello:** al cambiar un precio, su fondo se tiñe un instante del verde o rojo de dato al 20 % y se apaga en 0.9 s; la variación lleva signo y flecha.
- **Gráfico:** fondo `surface`, rejilla `line`, texto `ink-2` y velas y volumen en `up` y `down`, leídos de los tokens.

## Do's and Don'ts

### Do:
- **Do** abrir cada bloque con un filete de tinta de 3 px y separar cada fila con un filete de 1 px en `line`.
- **Do** usar los tokens de `tokens.css` por su rol, nunca un hex fijo, y comprobar cada superficie nueva en claro y en oscuro.
- **Do** poner cada cifra en tabulares, alineada a la derecha de su columna, con su procedencia cerca y su referencia cuando tenga fuente.
- **Do** resaltar invirtiendo tinta y papel; dentro de una zona invertida, invertir también la selección y el foco.
- **Do** colorear todo veredicto por su dirección: verde comprar y acumular, rojo reducir y evitar, tinta mantener y esperar.
- **Do** acompañar cada color de dato con texto, signo o flecha, y cada cuadrado de estado con su texto.
- **Do** llevar avisos y errores en ámbar sobre `warn-soft`; el error abre con filete de 3 px de ámbar.
- **Do** rellenar sin filete los chips estáticos y dar filete de tinta a los que se pulsan.
- **Do** dar a todo lo enfocable un anillo de 3 px en `focus` con esquinas rectas; hacia dentro donde el contenedor recorta y en `accent-ink` sobre tinta.
- **Do** dimensionar lo monumental y las cifras contra su columna y su número de caracteres, para que nada desborde.
- **Do** detener la cinta con «reducir movimiento», al señalarla, al enfocarla y con su botón de pausa.

### Don't:
- **Don't** usar el rojo señal fuera de la marca del raíl y el bloque de acción: ni texto, ni filetes, ni bordes, ni veredictos, ni errores.
- **Don't** usar rojo para un aviso, un error o una acción de borrar: en este mundo el rojo significa bajada.
- **Don't** introducir un color de acento, azul ni de ningún otro tono, para acciones, enlaces, selección o foco: es tinta.
- **Don't** añadir sombras, degradados, desenfoques ni esquinas redondeadas.
- **Don't** introducir una segunda familia tipográfica.
- **Don't** añadir iconos decorativos ni usar círculos como marca de estado: solo iconos lineales funcionales junto a su texto o con nombre accesible, y el cuadrado.
- **Don't** usar el estilo de etiqueta como antetítulo sobre un titular: solo nombra un dato.
- **Don't** centrar titulares ni bloques de texto.
- **Don't** añadir un movimiento continuo que no informe de un estado: la cinta es el único.
- **Don't** fijar un color de tema en un componente: los dos temas salen del mismo contrato.
