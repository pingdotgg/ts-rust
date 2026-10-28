import { Body, Controller, Get, HttpCode, HttpStatus, Param, ParseIntPipe, Post, Query } from '@nestjs/common';
import { ApiTags } from '@nestjs/swagger';
import { CatsService } from '@app/cats/cats.service';
import { Cat } from '@app/cats/entities/cat.entity';
import { PaginationQuery } from '@common';
import { CreateCatDto } from 'src/cats/dto/create-cat.dto';

@ApiTags('cats')
@Controller('cats')
export class CatsController {
  constructor(private readonly catsService: CatsService) {}

  /**
   * Create a cat.
   */
  @Post()
  @HttpCode(HttpStatus.CREATED)
  create(@Body() createCatDto: CreateCatDto): Cat {
    return this.catsService.create(createCatDto);
  }

  /**
   * List cats.
   */
  @Get()
  async findAll(@Query() query: PaginationQuery): Promise<Cat[]> {
    return this.catsService.findAll(query);
  }

  /**
   * Find one cat by id.
   */
  @Get(':id')
  findOne(@Param('id', ParseIntPipe) id: number): Cat {
    return this.catsService.findOne(id);
  }
}
