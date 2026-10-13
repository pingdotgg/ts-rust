import { IsInt, IsOptional, Max, Min } from 'class-validator';

export class PaginationQuery {
  /**
   * Number of items to skip.
   * @example 0
   */
  @IsOptional()
  @IsInt()
  @Min(0)
  offset?: number = 0;

  /**
   * Maximum number of items to return.
   */
  @IsOptional()
  @IsInt()
  @Min(1)
  @Max(100)
  limit?: number = 20;
}
