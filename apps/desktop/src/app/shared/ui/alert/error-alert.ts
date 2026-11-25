import { Component, computed, input } from '@angular/core';
import { TranslocoDirective } from '@jsverse/transloco';
import { AppError } from '../../../core/backend/backend.models';
import { Alert } from './alert';

/**
 * Alerta de falha de uma operação. Traduz o `code` estável do `AppError`; qualquer outro erro vira
 * uma mensagem genérica, sem mostrar o texto original.
 */
@Component({
  selector: 'app-error-alert',
  imports: [Alert, TranslocoDirective],
  template: `
    <ng-container *transloco="let t">
      <app-alert
        tone="danger"
        [title]="title()"
        [code]="code()"
        [codeLabel]="t('errors.codeLabel')"
      >
        {{ t('errors.' + key() + '.title') }}. {{ t('errors.' + key() + '.hint') }}
        {{ note() }}
      </app-alert>
    </ng-container>
  `,
})
export class ErrorAlert {
  readonly title = input.required<string>();
  readonly error = input.required<unknown>();
  readonly note = input('');

  protected readonly code = computed(() => {
    const error = this.error();
    return error instanceof AppError ? error.code : null;
  });
  protected readonly key = computed(() => this.code() ?? 'unknown');
}
