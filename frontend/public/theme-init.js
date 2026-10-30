/* Fija el tema guardado antes del primer pintado, para que con el oscuro elegido no haya un destello claro.
   Misma lógica que `src/app/theme.ts` (`read` y `applyTheme`): solo «light» y «dark» ponen `data-theme`;
   con cualquier otro valor (o sin almacenamiento) manda `prefers-color-scheme`. Va como fichero propio
   porque la CSP del backend (`script-src 'self'`) no admite scripts en línea. */
try {
  const theme = localStorage.getItem('invest-ai.theme') // const: queda en el bloque, sin global
  if (theme === 'light' || theme === 'dark') document.documentElement.setAttribute('data-theme', theme)
} catch (e) {
  /* sin almacenamiento: queda el tema del sistema */
}
