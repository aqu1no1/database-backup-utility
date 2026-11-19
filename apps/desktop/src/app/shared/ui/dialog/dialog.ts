import { Component, ElementRef, effect, input, model, viewChild } from '@angular/core';

let nextId = 0;

/**
 * Diálogo modal sobre o `<dialog>` nativo, que já prende o foco e fecha com Esc. Abre e fecha
 * por `open`; fechar pelo navegador (Esc) atualiza `open`. O corpo vem do conteúdo projetado e as
 * ações do slot `[dialog-actions]`.
 */
@Component({
  selector: 'app-dialog',
  template: `
    <dialog
      #dialog
      [attr.aria-labelledby]="titleId"
      [attr.aria-describedby]="description() ? descriptionId : null"
      class="m-auto box-border w-full max-w-[620px] rounded-[18px] border border-border bg-surface p-8 text-fg backdrop:bg-backdrop/85"
      (close)="open.set(false)"
    >
      <div class="flex flex-col gap-5">
        <div class="flex flex-col gap-2">
          <h2 [id]="titleId" class="m-0 text-2xl font-bold tracking-[-0.01em]">{{ title() }}</h2>
          @if (description(); as text) {
            <p [id]="descriptionId" class="m-0 text-[15px] leading-normal text-fg-muted">
              {{ text }}
            </p>
          }
        </div>
        <ng-content />
        <div class="flex flex-wrap justify-end gap-3 empty:hidden">
          <ng-content select="[dialog-actions]" />
        </div>
      </div>
    </dialog>
  `,
})
export class Dialog {
  readonly open = model(false);
  readonly title = input.required<string>();
  readonly description = input<string | null>(null);

  protected readonly titleId = `app-dialog-title-${nextId}`;
  protected readonly descriptionId = `app-dialog-description-${nextId++}`;

  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  constructor() {
    effect(() => {
      const dialog = this.dialog().nativeElement;
      if (this.open() && !dialog.open) {
        dialog.showModal();
      } else if (!this.open() && dialog.open) {
        dialog.close();
      }
    });
  }
}
