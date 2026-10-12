/** `npm run dev:mock` define VITE_MOCK=1. En producción esta constante se resuelve a `false` en el build. */
export const IS_MOCK: boolean = import.meta.env.VITE_MOCK === '1' || import.meta.env.VITE_MOCK === 'true'
