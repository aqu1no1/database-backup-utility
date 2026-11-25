import { Component, input, model } from '@angular/core';

let nextId = 0;

/**
 * Caixa de seleção com rótulo. Desabilitada, mostra sempre o motivo logo abaixo (`reason`),
 * ligado ao checkbox por `aria-describedby`, em vez de só ficar esmaecida.
 */
@Component({
  selector: 'app-check',
  host: { class: 'flex flex-col gap-1.5' },
  template: `
    <label
      class="flex items-center gap-2.5 text-[15px]"
      [class.text-fg-disabled]="disabled()"
      [class.cursor-not-allowed]="disabled()"
    >
      <input
        type="checkbox"
        [checked]="checked()"
        [disabled]="disabled()"
        [attr.aria-describedby]="reason() ? reasonId : null"
        (change)="onChange($event)"
      />
      {{ label() }}
    </label>
    @if (reason(); as text) {
      <span [id]="reasonId" class="pl-7 text-[13px] text-fg-subtle">{{ text }}</span>
    }
  `,
})
export class Check {
  readonly label = input.required<string>();
  readonly checked = model(false);
  readonly disabled = input(false);
  readonly reason = input<string | null>(null);

  protected readonly reasonId = `app-check-${nextId++}`;

  protected onChange(event: Event): void {
    this.checked.set((event.target as HTMLInputElement).checked);
  }
}
