import { Component, computed, input, model } from '@angular/core';

export interface SegmentedOption<T extends string> {
  readonly value: T;
  readonly label: string;
  readonly hint?: string;
}

export type SegmentedAppearance = 'segmented' | 'chips';

let nextId = 0;

/**
 * Escolha de uma opção entre poucas, com botões `aria-pressed`. `segmented` é a barra do design
 * (tema, idioma, executor, TLS); `chips` são os filtros em pílula do histórico. A dica da opção
 * escolhida aparece logo abaixo, quando existir.
 */
@Component({
  selector: 'app-segmented',
  host: { class: 'flex flex-col gap-2' },
  template: `
    <div
      role="group"
      [attr.aria-label]="label()"
      [attr.aria-describedby]="selectedHint() ? hintId : null"
      [class]="groupClasses()"
    >
      @for (option of options(); track option.value) {
        <button
          type="button"
          [attr.aria-pressed]="option.value === value()"
          [class]="buttonClasses(option.value === value())"
          (click)="value.set(option.value)"
        >
          {{ option.label }}
        </button>
      }
    </div>
    @if (selectedHint(); as hint) {
      <span [id]="hintId" class="text-[13px] text-fg-subtle">{{ hint }}</span>
    }
  `,
})
export class Segmented<T extends string> {
  readonly options = input.required<readonly SegmentedOption<T>[]>();
  readonly value = model.required<T>();
  readonly label = input.required<string>();
  readonly appearance = input<SegmentedAppearance>('segmented');

  protected readonly hintId = `app-segmented-${nextId++}`;

  protected readonly selectedHint = computed(
    () => this.options().find((option) => option.value === this.value())?.hint ?? null,
  );

  protected readonly groupClasses = computed(() =>
    this.appearance() === 'chips'
      ? 'flex flex-wrap gap-2'
      : 'grid auto-cols-fr grid-flow-col gap-1 rounded-[10px] border border-border bg-canvas p-1',
  );

  protected buttonClasses(active: boolean): string {
    if (this.appearance() === 'chips') {
      return active
        ? 'min-h-10 cursor-pointer rounded-full border-0 bg-primary px-4 text-sm font-semibold text-on-primary'
        : 'min-h-10 cursor-pointer rounded-full border border-border bg-transparent px-4 text-sm text-fg';
    }
    return active
      ? 'min-h-10 cursor-pointer rounded-[7px] border-0 bg-primary px-3 text-sm font-bold text-on-primary'
      : 'min-h-10 cursor-pointer rounded-[7px] border-0 bg-transparent px-3 text-sm font-medium text-fg-muted';
  }
}
