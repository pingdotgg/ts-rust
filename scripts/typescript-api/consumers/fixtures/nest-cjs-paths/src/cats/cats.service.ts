import { Injectable, NotFoundException } from '@nestjs/common';
import { CatColor, PaginationQuery } from '@common';
import type { Cat } from '@app/cats/entities/cat.entity';
import { CreateCatDto } from 'src/cats/dto/create-cat.dto';

@Injectable()
export class CatsService {
  private readonly cats: Cat[] = [
    {
      id: 1,
      name: 'Kitty',
      age: 3,
      breed: 'Maine Coon',
      color: CatColor.Ginger,
      tags: ['fluffy'],
      createdAt: new Date('2026-01-01T00:00:00Z'),
    },
  ];

  create(dto: CreateCatDto): Cat {
    const cat: Cat = { id: this.cats.length + 1, ...dto, createdAt: new Date('2026-01-02T00:00:00Z') };
    this.cats.push(cat);
    return cat;
  }

  findAll({ offset = 0, limit = 20 }: PaginationQuery): Cat[] {
    return this.cats.slice(offset, offset + limit);
  }

  findOne(id: number): Cat {
    const cat = this.cats.find((candidate) => candidate.id === id);
    if (!cat) {
      throw new NotFoundException(`Cat ${id} not found`);
    }
    return cat;
  }
}
