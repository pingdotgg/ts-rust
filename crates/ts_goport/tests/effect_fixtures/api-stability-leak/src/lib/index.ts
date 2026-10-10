/** @stability unstable */
export interface Unstable {
  value: string
}

/** @stability experimental */
export interface Experimental {
  value: string
}

export interface StableLeak {
  member: Unstable
}

export type StableGeneric = Array<Experimental>

export declare function make(input: Unstable): Experimental

/** @stability unstable */
export interface UnstableLeak {
  member: Experimental
}

/** @stability unstable */
export interface UnstableOk {
  member: Unstable
}

/** @stability experimental */
export interface ExperimentalOk {
  member: Experimental
}

/** @internal */
export interface Hidden {
  member: Experimental
}

export class Service {
  private secret!: Experimental
  protected guarded!: Unstable
  public exposed!: Unstable
}

/** @stability experimental */
export class TaggedBase {
  base!: string
}

export class Derived extends TaggedBase {}

export namespace Api {
  export interface Shown {
    value: Unstable
  }
  /** @internal */
  export interface HiddenInside {
    value: Experimental
  }
}
