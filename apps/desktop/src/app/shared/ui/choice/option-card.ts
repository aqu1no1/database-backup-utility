import { Component, computed, input, output } from '@angular/core';

let nextId = 0;

/**
 * Card de opção com rádio (por exemplo o destino da restauração). Selecionado, ganha borda de
 * destaque; desabilitado, fica tracejado e mostra o motivo logo abaixo. Detalhes extras da opção
 * vêm do conteúdo projetado.
 */
@Component({
  selector: 'app-option-card',
  host: {
    class: 'flex flex-col gap-1.5 rounded-[14px]',
    '[class]': 'stateClasses()',
  },
  template: `
    <label
      class="flex items-center gap-2.5 text-[15px] font-semibold"
      [class.text-fg-muted]="!selected() && !disabled()"
      [class.text-fg-disabled]="disabled()"
      [class.cursor-pointer]="!disabled()"
      [class.cursor-not-allowed]="disabled()"
    >
      <input
        type="radio"
        [name]="name()"
        [checked]="selected()"
        [disabled]="disabled()"
        [attr.aria-describedby]="reason() ? reasonId : null"
        (change)="choose.emit()"
      />
      {{ label() }}
    </label>
    <div class="flex flex-col gap-1.5 pl-7 text-sm text-fg-muted empty:hidden"><ng-content /></div>
    @if (disabled() && reason(); as text) {
      <span [id]="reasonId" class="pl-7 text-[13px] text-fg-subtle">{{ text }}</span>
    }
  `,
})
export class OptionCard {
  readonly name = input.required<string>();
  readonly label = input.required<string>();
  readonly selected = input(false);
  readonly disabled = input(false);
  readonly reason = input<string | null>(null);
  readonly choose = output();

  protected readonly reasonId = `app-option-card-${nextId++}`;

  protected readonly stateClasses = computed(() => {
    if (this.disabled()) {
      return 'border border-dashed border-border px-[17px] py-[15px]';
    }
    return this.selected()
      ? 'border-2 border-primary bg-canvas px-[17px] py-[15px]'
      : 'border border-border bg-canvas px-[18px] py-4';
  });
}
