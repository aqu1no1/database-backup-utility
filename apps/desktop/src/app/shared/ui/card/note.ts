import { Component, input } from '@angular/core';

/**
 * Nota com borda tracejada para explicações e itens futuros. O texto vem do conteúdo projetado.
 */
@Component({
  selector: 'app-note',
  host: {
    class: 'flex flex-col gap-1.5 rounded-[14px] border border-dashed border-border p-[18px]',
  },
  template: `
    @if (title(); as text) {
      <span class="text-[15px] font-semibold">{{ text }}</span>
    }
    <span class="text-sm leading-normal text-fg-muted"><ng-content /></span>
  `,
})
export class Note {
  readonly title = input<string | null>(null);
}
