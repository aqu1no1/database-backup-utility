import { Component, computed, input } from '@angular/core';

export type StatusTone = 'success' | 'running' | 'warning' | 'danger' | 'planned';

const TONES: Record<StatusTone, string> = {
  success: 'bg-success-bg text-success-fg',
  running: 'bg-neutral text-fg',
  warning: 'bg-warning-bg text-warning-fg',
  danger: 'bg-danger-bg text-danger-fg',
  planned: 'border border-border text-fg-subtle',
};

const SYMBOLS: Record<StatusTone, string> = {
  success: '✓',
  running: '●',
  warning: '!',
  danger: '✕',
  planned: '',
};

/**
 * Pílula de status de uma operação (verificado, executando, atenção, falhou, planejado). O texto
 * vem do conteúdo projetado; o símbolo é decorativo e fica oculto para leitores de tela.
 */
@Component({
  selector: 'app-status-badge',
  host: {
    class:
      'inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-1 text-[13px] font-semibold',
    '[class]': 'toneClasses()',
  },
  template: `@if (symbol()) {
      <span aria-hidden="true">{{ symbol() }}</span>
    }
    <ng-content />`,
})
export class StatusBadge {
  readonly tone = input.required<StatusTone>();

  protected readonly toneClasses = computed(() => TONES[this.tone()]);
  protected readonly symbol = computed(() => SYMBOLS[this.tone()]);
}
