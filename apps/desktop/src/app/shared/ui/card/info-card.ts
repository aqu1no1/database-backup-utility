import { Component, input } from '@angular/core';

/**
 * Card de informação: rótulo, título e um dado técnico em fonte mono (endereço, caminho, formato).
 */
@Component({
  selector: 'app-info-card',
  host: { class: 'card flex flex-col gap-2 p-5' },
  template: `
    <span class="text-[13px] text-fg-subtle">{{ label() }}</span>
    <span class="text-[17px] font-semibold">{{ title() }}</span>
    @if (detail(); as text) {
      <span class="font-mono text-[13px] [overflow-wrap:anywhere] text-fg-muted">{{ text }}</span>
    }
  `,
})
export class InfoCard {
  readonly label = input.required<string>();
  readonly title = input.required<string>();
  readonly detail = input<string | null>(null);
}
