/** @stability experimental */
export const beta = (): number => 1

/** @stability experimental */
export const epsilon = (): number => 5

/** @stability unstable */
export const gamma = 2

export interface Box {
  /** @stability experimental */
  open(): number
  close(): number
}

/** @stability experimental */
export function over(a: string): string
export function over(a: number): number
export function over(a: unknown): unknown {
  return a
}

export const dual: {
  /** @stability experimental */
  (a: string): string
  (a: number): number
} = (a: any) => a

/** @stability experimental */
export class Widget {
  static create(): Widget {
    return new Widget()
  }
}
