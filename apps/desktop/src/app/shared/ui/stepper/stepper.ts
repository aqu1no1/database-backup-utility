import { Component, input } from '@angular/core';

export type StepState = 'done' | 'current' | 'next';

export interface Step {
  readonly label: string;
  readonly state: StepState;
}

/**
 * Etapas numeradas de um fluxo (por exemplo o wizard de restauração). A etapa atual recebe
 * `aria-current="step"`; as concluídas mostram ✓ no lugar do número.
 */
@Component({
  selector: 'app-stepper',
  template: `
    <ol [attr.aria-label]="label()" class="m-0 flex list-none flex-wrap gap-5 p-0 text-sm">
      @for (step of steps(); track $index) {
        <li
          class="flex items-center gap-2.5"
          [class.text-fg-muted]="step.state === 'done'"
          [class.font-semibold]="step.state === 'current'"
          [class.text-fg-subtle]="step.state === 'next'"
          [attr.aria-current]="step.state === 'current' ? 'step' : null"
        >
          <span
            aria-hidden="true"
            class="box-border flex size-7 items-center justify-center rounded-full text-[13px]"
            [class.bg-success-bg]="step.state === 'done'"
            [class.text-success-fg]="step.state === 'done'"
            [class.font-bold]="step.state !== 'next'"
            [class.bg-primary]="step.state === 'current'"
            [class.text-on-primary]="step.state === 'current'"
            [class.border]="step.state === 'next'"
            [class.border-border]="step.state === 'next'"
            >{{ step.state === 'done' ? '✓' : $index + 1 }}</span
          >
          {{ step.label }}
        </li>
      }
    </ol>
  `,
})
export class Stepper {
  readonly steps = input.required<readonly Step[]>();
  readonly label = input.required<string>();
}
