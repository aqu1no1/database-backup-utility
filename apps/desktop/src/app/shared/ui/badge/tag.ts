import { Component, input } from '@angular/core';

export type TagIcon = 'encrypted' | 'compressed';

/**
 * Etiqueta discreta com contorno, usada para propriedades de um backup (criptografado,
 * comprimido) e para o SGBD de uma conexão. O texto vem do conteúdo projetado.
 */
@Component({
  selector: 'app-tag',
  host: {
    class:
      'inline-flex items-center gap-1.5 whitespace-nowrap rounded-full border border-border px-[9px] py-[3px] text-xs font-semibold text-fg-muted',
  },
  template: `
    @switch (icon()) {
      @case ('encrypted') {
        <svg
          viewBox="0 0 24 24"
          width="13"
          height="13"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="5" y="11" width="14" height="9" rx="2" />
          <path d="M8 11V8a4 4 0 0 1 8 0v3" />
        </svg>
      }
      @case ('compressed') {
        <svg
          viewBox="0 0 24 24"
          width="13"
          height="13"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7" />
        </svg>
      }
    }
    <ng-content />
  `,
})
export class Tag {
  readonly icon = input<TagIcon | null>(null);
}
