import { Component, input } from '@angular/core';

/**
 * Card de métrica do painel: rótulo pequeno e valor em destaque. O valor e uma linha de detalhe
 * opcional vêm do conteúdo projetado.
 */
@Component({
  selector: 'app-metric-card',
  host: { class: 'card flex flex-col gap-1.5 p-5' },
  template: `
    <span class="text-[13px] text-fg-subtle">{{ label() }}</span>
    <span class="text-[26px] font-bold"><ng-content /></span>
    <ng-content select="[detail]" />
  `,
})
export class MetricCard {
  readonly label = input.required<string>();
}
