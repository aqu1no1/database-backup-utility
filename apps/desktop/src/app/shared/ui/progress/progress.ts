import { Component, computed, input } from '@angular/core';

/**
 * Barra de progresso. Com `value` (0 a 100) é determinada; com `value` nulo é indeterminada, para
 * o dump em streaming, que não tem total conhecido: mostra a etapa (`label`) e o andamento em
 * texto (`valueText`, por exemplo os bytes gravados). Respeita `prefers-reduced-motion`.
 */
@Component({
  selector: 'app-progress',
  host: { class: 'flex flex-col gap-2' },
  template: `
    <div class="flex justify-between gap-3 text-sm">
      <span class="text-fg-muted">{{ label() }}</span>
      <span class="font-mono">{{ valueText() }}</span>
    </div>
    <div
      role="progressbar"
      class="h-2 overflow-hidden rounded bg-canvas"
      [attr.aria-label]="label()"
      [attr.aria-valuemin]="determinate() ? 0 : null"
      [attr.aria-valuemax]="determinate() ? 100 : null"
      [attr.aria-valuenow]="clamped()"
      [attr.aria-valuetext]="valueText()"
    >
      @if (determinate()) {
        <div class="h-full rounded bg-primary" [style.width.%]="clamped()"></div>
      } @else {
        <div class="animate-indeterminate h-full w-2/5 rounded bg-primary"></div>
      }
    </div>
  `,
})
export class Progress {
  readonly label = input.required<string>();
  readonly value = input<number | null>(null);
  readonly valueText = input.required<string>();

  protected readonly determinate = computed(() => this.value() !== null);
  protected readonly clamped = computed(() => {
    const value = this.value();
    return value === null ? null : Math.min(100, Math.max(0, value));
  });
}
