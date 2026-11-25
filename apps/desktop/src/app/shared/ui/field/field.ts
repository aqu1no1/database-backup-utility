import { Component, computed, contentChild, effect, input } from '@angular/core';
import { Control } from './control';

let nextId = 0;

/**
 * Rótulo, controle projetado (com `appControl`), texto de ajuda e mensagem de erro. Com erro, a
 * ajuda é substituída pela mensagem, o controle fica com borda de erro e `aria-invalid`.
 */
@Component({
  selector: 'app-field',
  host: { class: 'flex flex-col gap-2' },
  template: `
    <label class="text-sm font-medium" [for]="control()?.id()">{{ label() }}</label>
    <ng-content />
    @if (error(); as message) {
      <span [id]="messageId" class="text-[13px] text-danger-fg">{{ message }}</span>
    } @else if (help(); as text) {
      <span [id]="messageId" class="text-[13px] text-fg-subtle">{{ text }}</span>
    }
  `,
})
export class Field {
  readonly label = input.required<string>();
  readonly help = input<string | null>(null);
  readonly error = input<string | null>(null);

  protected readonly messageId = `app-field-${nextId++}`;

  protected readonly control = contentChild(Control);
  private readonly hasMessage = computed(() => !!this.error() || !!this.help());

  constructor() {
    effect(() => {
      this.control()?.attach(!!this.error(), this.hasMessage() ? this.messageId : null);
    });
  }
}
