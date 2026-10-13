import { Controller, Get } from '@nestjs/common';
import { AppService } from '@app/app.service';

@Controller()
export class AppController {
  constructor(private readonly appService: AppService) {}

  /**
   * Greeting with the build marker that the watch scenario edits.
   */
  @Get()
  getHello(): string {
    return this.appService.getHello();
  }
}
