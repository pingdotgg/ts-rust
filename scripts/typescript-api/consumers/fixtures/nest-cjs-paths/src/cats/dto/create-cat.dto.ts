import { IsArray, IsEnum, IsInt, IsOptional, IsString, Max, Min } from 'class-validator';
import { CatColor, CatSize } from '@common';

export class CreateCatDto {
  /**
   * The name of the cat.
   * @example Kitty
   */
  @IsString()
  readonly name: string;

  /**
   * Age in years.
   */
  @IsInt()
  @Min(0)
  @Max(40)
  readonly age: number;

  @IsString()
  readonly breed: string;

  @IsEnum(CatColor)
  readonly color: CatColor = CatColor.Tabby;

  @IsOptional()
  @IsEnum(CatSize)
  readonly size?: CatSize;

  @IsArray()
  @IsString({ each: true })
  readonly tags: string[] = [];
}
