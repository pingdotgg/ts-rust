class A { static readonly #a = Symbol(); static t() { const n: number = A.#a; } }
const broken = ;
