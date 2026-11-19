import { Directive, booleanAttribute, computed, input, signal } from '@angular/core';

let nextId = 0;

/**
 * Visual de campo do design para `<input>`, `<select>` e `<textarea>` nativos. Dentro de um
 * `app-field`, recebe dele o estado de erro e o `aria-describedby` da ajuda ou do erro, e o
 * rótulo do campo aponta para o `id` deste controle.
 */
@Directive({
  selector: 'input[appControl], select[appControl], textarea[appControl]',
  host: {
    class:
      'control w-full min-w-0 focus:border-primary focus:shadow-[inset_0_0_0_1px_var(--color-primary)] focus:outline-none',
    '[id]': 'id()',
    '[class.font-mono]': 'mono()',
    '[class.border-danger-fg]': 'invalid()',
    '[attr.aria-invalid]': 'invalid() ? "true" : null',
    '[attr.aria-describedby]': 'describedBy()',
  },
})
export class Control {
  readonly id = input(`app-control-${nextId++}`);
  readonly mono = input(false, { transform: booleanAttribute });

  private readonly fieldError = signal(false);
  private readonly fieldDescription = signal<string | null>(null);

  protected readonly invalid = computed(() => this.fieldError());
  protected readonly describedBy = computed(() => this.fieldDescription());

  attach(invalid: boolean, describedBy: string | null): void {
    this.fieldError.set(invalid);
    this.fieldDescription.set(describedBy);
  }
}
