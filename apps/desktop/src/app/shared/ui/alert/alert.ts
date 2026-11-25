import { Component, computed, input } from '@angular/core';

export type AlertTone = 'danger' | 'warning';

/**
 * Alerta de erro ou de atenção. O erro é anunciado (`role="alert"`) e pode mostrar o `code`
 * estável vindo do motor Rust, prefixado por `codeLabel`. A mensagem vem do conteúdo projetado e
 * nunca deve conter credenciais.
 */
@Component({
  selector: 'app-alert',
  host: {
    class: 'flex flex-col gap-1.5 rounded-[14px] border p-[18px]',
    '[class]': 'toneClasses()',
    '[attr.role]': 'tone() === "danger" ? "alert" : null',
  },
  template: `
    <span class="text-base font-semibold text-fg">{{ title() }}</span>
    <span
      class="text-sm"
      [class.text-danger-text]="tone() === 'danger'"
      [class.text-warning-text]="tone() === 'warning'"
      ><ng-content
    /></span>
    @if (code() && codeLabel()) {
      <span class="font-mono text-[13px] text-danger-fg">{{ codeLabel() }}: {{ code() }}</span>
    }
  `,
})
export class Alert {
  readonly tone = input.required<AlertTone>();
  readonly title = input.required<string>();
  readonly code = input<string | null>(null);
  readonly codeLabel = input<string | null>(null);

  protected readonly toneClasses = computed(() =>
    this.tone() === 'danger'
      ? 'border-danger-border bg-danger-surface'
      : 'border-warning-border bg-warning-surface',
  );
}
