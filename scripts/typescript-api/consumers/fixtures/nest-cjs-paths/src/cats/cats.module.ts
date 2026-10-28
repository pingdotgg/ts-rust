import { Module } from '@nestjs/common';
import { CatsController } from './cats.controller';
import { CatsService } from '@app/cats/cats.service';

@Module({
  controllers: [CatsController],
  providers: [CatsService],
  exports: [CatsService],
})
export class CatsModule {}
