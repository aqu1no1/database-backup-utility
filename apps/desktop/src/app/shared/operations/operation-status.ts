import { Component, computed, input } from '@angular/core';
import { TranslocoDirective } from '@jsverse/transloco';
import { OperationStatus } from '../../core/backend/backend.models';
import { StatusBadge, StatusTone } from '../ui/badge/status-badge';

const TONES: Record<OperationStatus, StatusTone> = {
  running: 'running',
  verified: 'success',
  completed: 'success',
  failed: 'danger',
  cancelled: 'warning',
};

/** Status traduzido de um backup ou de uma restauração. */
@Component({
  selector: 'app-operation-status',
  imports: [StatusBadge, TranslocoDirective],
  template: `
    <app-status-badge *transloco="let t" [tone]="tone()">{{
      t('operations.status.' + status())
    }}</app-status-badge>
  `,
})
export class OperationStatusBadge {
  readonly status = input.required<OperationStatus>();

  protected readonly tone = computed(() => TONES[this.status()]);
}
