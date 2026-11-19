import { Directive, computed, input } from '@angular/core';

export type ButtonVariant = 'primary' | 'secondary' | 'destructive' | 'text';
export type ButtonSize = 'md' | 'sm';

const BASE =
  'inline-flex items-center justify-center gap-2 whitespace-nowrap no-underline cursor-pointer disabled:cursor-not-allowed';

const SIZES: Record<ButtonSize, string> = {
  md: 'min-h-11 rounded-[10px] text-[15px]',
  sm: 'min-h-9 rounded-lg px-3.5 text-sm',
};

const VARIANTS: Record<ButtonVariant, string> = {
  primary:
    'border-0 bg-primary font-bold text-on-primary hover:text-on-primary disabled:bg-neutral disabled:text-fg-subtle',
  secondary:
    'border border-border bg-transparent font-medium text-fg hover:text-fg-strong disabled:text-fg-disabled',
  destructive:
    'border-0 bg-warning-solid font-bold text-on-warning hover:text-on-warning disabled:bg-neutral disabled:text-fg-subtle',
  text: 'border-0 bg-transparent text-fg-subtle hover:text-fg',
};

const PADDING: Record<ButtonVariant, string> = {
  primary: 'px-5',
  secondary: 'px-[18px]',
  destructive: 'px-5',
  text: 'px-4 text-sm',
};

/**
 * Aplica o visual de botão do design a um `<button>` ou `<a>` nativo, mantendo a semântica do
 * elemento. `destructive` é reservado para ações que apagam ou sobrescrevem dados.
 */
@Directive({
  selector: 'button[appButton], a[appButton]',
  host: { '[class]': 'classes()' },
})
export class Button {
  readonly variant = input<ButtonVariant>('secondary', { alias: 'appButton' });
  readonly size = input<ButtonSize>('md');

  protected readonly classes = computed(() => {
    const variant = this.variant();
    const size = this.size();
    return [BASE, SIZES[size], VARIANTS[variant], size === 'md' ? PADDING[variant] : ''].join(' ');
  });
}
