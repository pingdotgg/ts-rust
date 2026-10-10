import { Kind, Flags } from "./kinds";
import type { Shape } from "./shape";

function column(name?: string): PropertyDecorator {
  return () => {};
}

function entity(target: Function) {}

/** An entity with decorated members. */
@entity
export class Model {
  @column()
  id: number = 0;

  @column("label")
  label: Kind = Kind.Box;

  #secret = Flags.Read | Flags.Write;

  constructor(public shape?: Shape) {}

  @column()
  get secret(): number {
    return this.#secret;
  }

  async *items(): AsyncGenerator<number> {
    for await (const item of [1, 2, 3]) {
      yield item ?? this.#secret;
    }
  }

  merge(other: Partial<Model>): Partial<Model> & { size: number } {
    return { ...this, ...other, size: other.shape?.size ?? 0 };
  }
}
