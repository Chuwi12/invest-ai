---
version: 1
slug: "frontend-src-app-shell-tsx"
primary_target: "frontend/src/app/Shell.tsx"
related_targets: []
---

# Panel (todas las rutas dentro del Shell)

## Alcance y modo
Operate. Panel, ficha de activo e informe, chat, favoritos, historial, rendimiento, OPV y modelos. Hereda el mundo «Extracto suizo» de DESIGN.md (decisión del usuario, 27-oct-2026); la composición, el marcado y el comportamiento de cada página no cambian.

## Decisiones del usuario
- Una sola familia en toda la app: Archivo (se quitaron Geist y Newsreader).
- Acciones, selección y foco en tinta (negro en claro, blanco en oscuro); el rojo queda para bajadas, veredictos y la marca, porque en el panel el rojo significa «baja».
- Modo día y noche en landing y panel con el conmutador existente (sistema → claro → oscuro, `app/theme.ts`).
- Prioridad: no perjudicar la funcionalidad ni el rendimiento (cambio de tokens y de reglas de componente, sin marcado nuevo, sin dependencias, sin movimiento nuevo).

## Contrato de tokens
`frontend/src/styles/tokens.css` (claro y oscuro, mismos nombres que usaba app.css). Nadie más lo edita sin el coordinador.

## Lo que debe sentirse
Un extracto: papel y tinta, filetes de 3 px que abren bloques y de 1 px entre filas, esquinas rectas, sin sombras ni degradados, cifras tabulares, bandera a la izquierda. En Operate la expresión vive en los detalles: escaneabilidad y densidad de tablas por encima del gesto.

## Pendiente de decidir
- Nada abierto.
