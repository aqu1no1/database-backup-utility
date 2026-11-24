import { Pipe, PipeTransform, inject } from '@angular/core';
import { LanguageService } from '../../core/i18n/language.service';
import { shortKey } from '../utils/age-recipient';
import { formatBytes, formatDateTime, formatDuration } from '../utils/format';

@Pipe({ name: 'fileSize', pure: false })
export class FileSizePipe implements PipeTransform {
  private readonly language = inject(LanguageService).language;

  transform(bytes: number): string {
    return formatBytes(bytes, this.language());
  }
}

@Pipe({ name: 'duration', pure: false })
export class DurationPipe implements PipeTransform {
  private readonly language = inject(LanguageService).language;

  transform(milliseconds: number): string {
    return formatDuration(milliseconds, this.language());
  }
}

@Pipe({ name: 'dateTime', pure: false })
export class DateTimePipe implements PipeTransform {
  private readonly language = inject(LanguageService).language;

  transform(value: Date | string | number): string {
    return formatDateTime(value, this.language());
  }
}

@Pipe({ name: 'shortKey' })
export class ShortKeyPipe implements PipeTransform {
  transform(key: string): string {
    return shortKey(key);
  }
}
