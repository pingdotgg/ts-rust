import { Injectable } from '@nestjs/common';

export const BUILD_MARKER = 'v1';

@Injectable()
export class AppService {
  getHello(): string {
    return `Hello from nest-cjs-paths ${BUILD_MARKER}`;
  }
}
