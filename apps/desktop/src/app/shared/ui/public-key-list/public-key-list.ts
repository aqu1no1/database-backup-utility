import { Component, input, model, signal } from '@angular/core';
import { ShortKeyPipe } from '../../pipes/format.pipes';
import { checkRecipient, RecipientProblem } from '../../utils/age-recipient';
import { Button } from '../button/button';

let nextId = 0;

/**
 * Lista editável de chaves públicas age (destinatários de um backup criptografado). Valida o
 * formato `age1…` e duplicatas antes de adicionar; mostra cada chave encurtada, com a completa no
 * título. Chaves privadas nunca passam por aqui.
 */
@Component({
  selector: 'app-public-key-list',
  imports: [Button, ShortKeyPipe],
  host: { class: 'flex flex-col gap-3.5' },
  template: `
    @if (keys().length) {
      <ul class="m-0 flex list-none flex-col gap-2 p-0">
        @for (key of keys(); track key) {
          <li class="card-inset flex min-h-11 items-center gap-3 rounded-[10px] py-0 pr-1.5 pl-3.5">
            <span class="flex-auto font-mono text-sm" [title]="key">{{ key | shortKey }}</span>
            <button
              appButton="text"
              size="sm"
              type="button"
              [attr.aria-label]="removeLabel() + ' ' + (key | shortKey)"
              (click)="remove(key)"
            >
              {{ removeLabel() }}
            </button>
          </li>
        }
      </ul>
    }
    <div class="flex gap-2">
      <input
        #draft
        type="text"
        class="control flex-auto font-mono focus:border-primary focus:outline-none"
        [class.border-danger-fg]="problem()"
        [attr.aria-label]="inputLabel()"
        [attr.aria-invalid]="problem() ? 'true' : null"
        [attr.aria-describedby]="problem() ? errorId : null"
        placeholder="age1…"
        autocomplete="off"
        spellcheck="false"
        (input)="problem.set(null)"
        (keydown.enter)="add(draft)"
      />
      <button appButton="secondary" type="button" (click)="add(draft)">{{ addLabel() }}</button>
    </div>
    @if (problem(); as problem) {
      <span [id]="errorId" role="alert" class="text-[13px] text-danger-fg">{{
        problem === 'duplicate' ? duplicateMessage() : invalidMessage()
      }}</span>
    }
  `,
})
export class PublicKeyList {
  readonly keys = model.required<readonly string[]>();
  readonly inputLabel = input.required<string>();
  readonly addLabel = input.required<string>();
  readonly removeLabel = input.required<string>();
  readonly invalidMessage = input.required<string>();
  readonly duplicateMessage = input.required<string>();

  protected readonly problem = signal<RecipientProblem | null>(null);
  protected readonly errorId = `app-public-key-list-${nextId++}`;

  protected add(draft: HTMLInputElement): void {
    const candidate = draft.value.trim();
    const problem = checkRecipient(candidate, this.keys());
    if (problem) {
      this.problem.set(problem);
      return;
    }
    this.keys.set([...this.keys(), candidate]);
    draft.value = '';
  }

  protected remove(key: string): void {
    this.keys.set(this.keys().filter((item) => item !== key));
  }
}
