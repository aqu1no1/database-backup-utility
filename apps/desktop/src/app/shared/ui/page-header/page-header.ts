import { Component, input } from '@angular/core';

/**
 * Cabeçalho de página: sobretítulo opcional em mono (por exemplo a conexão ou o ID do backup),
 * título `h1` e ações à direita, vindas do conteúdo projetado.
 */
@Component({
  selector: 'app-page-header',
  host: { class: 'flex flex-wrap items-end justify-between gap-4' },
  template: `
    <div class="flex flex-col gap-1.5">
      @if (overline(); as text) {
        <span class="overline">{{ text }}</span>
      }
      <h1 class="m-0 text-[32px] leading-[1.1] font-bold tracking-[-0.02em]">{{ title() }}</h1>
    </div>
    <div class="flex flex-wrap gap-3 empty:hidden"><ng-content /></div>
  `,
})
export class PageHeader {
  readonly title = input.required<string>();
  readonly overline = input<string | null>(null);
}
