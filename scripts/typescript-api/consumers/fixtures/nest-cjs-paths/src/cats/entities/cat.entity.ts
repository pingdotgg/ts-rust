import { CatColor, CatSize } from '@common';

/**
 * A cat known to the service.
 */
export class Cat {
  /**
   * Unique id.
   * @example 1
   */
  id: number;

  /**
   * The name of the cat.
   * @example Kitty
   */
  name: string;

  /**
   * Age in years.
   */
  age: number;

  breed: string;

  color: CatColor;

  size?: CatSize;

  tags: string[];

  /**
   * When the cat was added.
   */
  createdAt: Date;

  /** @deprecated Use tags instead. */
  nickname?: string | null;
}
