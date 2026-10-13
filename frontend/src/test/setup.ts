import '@testing-library/jest-dom/vitest'
import { afterEach } from 'vitest'
import { cleanup, configure } from '@testing-library/react'

// Las esperas (`findBy*`, `waitFor`) terminan en cuanto se cumple la condición; el tope solo cuenta
// cuando algo no llega. El valor por defecto (1 s) es menor que lo que tardan un chunk perezoso o
// una respuesta del mock en una máquina cargada: con carga de CPU fallaba 1 de cada ~12 ejecuciones.
configure({ asyncUtilTimeout: 10_000 })

// jsdom no implementa scrollIntoView (el chat y la ficha lo llaman al pintar).
if (!Element.prototype.scrollIntoView) Element.prototype.scrollIntoView = () => undefined

afterEach(() => cleanup())
