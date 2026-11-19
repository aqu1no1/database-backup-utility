import { Component, input, output } from '@angular/core';
import { Button } from '../button/button';

/**
 * Mostra o caminho escolhido (ou o texto de vazio) e um botão que pede a escolha de um arquivo
 * ou pasta. Não abre diálogo nem lê o disco: só emite `pick`, e quem usa decide como escolher
 * (diálogo do Tauri ou backend simulado). Chaves privadas aparecem só pelo caminho.
 */
@Component({
  selector: 'app-file-picker',
  imports: [Button],
  host: { class: 'flex flex-col gap-2' },
  template: `
    <span class="text-sm font-medium">{{ label() }}</span>
    <div class="flex gap-2">
      <span
        class="control flex flex-auto items-center overflow-hidden font-mono text-ellipsis whitespace-nowrap"
        [class.text-fg-subtle]="!path()"
        [attr.title]="path()"
        >{{ path() ?? emptyLabel() }}</span
      >
      <button
        appButton="secondary"
        type="button"
        [attr.aria-label]="buttonLabel()"
        (click)="pick.emit()"
      >
        {{ path() ? changeLabel() : chooseLabel() }}
      </button>
    </div>
  `,
})
export class FilePicker {
  readonly label = input.required<string>();
  readonly path = input<string | null>(null);
  readonly emptyLabel = input.required<string>();
  readonly chooseLabel = input.required<string>();
  readonly changeLabel = input.required<string>();
  readonly pick = output();

  protected buttonLabel(): string {
    return `${this.path() ? this.changeLabel() : this.chooseLabel()} ${this.label()}`;
  }
}
