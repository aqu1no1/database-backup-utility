import { LOCALE_ID, Pipe, PipeTransform, inject } from '@angular/core';
import { shortKey } from '../utils/age-recipient';
import { formatBytes, formatDateTime, formatDuration } from '../utils/format';

@Pipe({ name: 'fileSize' })
export class FileSizePipe implements PipeTransform {
  private readonly locale = inject(LOCALE_ID);

  transform(bytes: number): string {
    return formatBytes(bytes, this.locale);
  }
}

@Pipe({ name: 'duration' })
export class DurationPipe implements PipeTransform {
  private readonly locale = inject(LOCALE_ID);

  transform(milliseconds: number): string {
    return formatDuration(milliseconds, this.locale);
  }
}

@Pipe({ name: 'dateTime' })
export class DateTimePipe implements PipeTransform {
  private readonly locale = inject(LOCALE_ID);

  transform(value: Date | string | number): string {
    return formatDateTime(value, this.locale);
  }
}

@Pipe({ name: 'shortKey' })
export class ShortKeyPipe implements PipeTransform {
  transform(key: string): string {
    return shortKey(key);
  }
}
