import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Control } from './control';
import { Field } from './field';

@Component({
  imports: [Field, Control],
  template: `
    <app-field label="Porta" [help]="help()" [error]="error()">
      <input appControl mono type="text" />
    </app-field>
  `,
})
class Host {
  readonly help = signal<string | null>('Padrão: 5432');
  readonly error = signal<string | null>(null);
}

describe('Field', () => {
  async function render() {
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;
    return { fixture, element, input: element.querySelector('input') as HTMLInputElement };
  }

  it('labels the projected control and links it to the help text', async () => {
    const { element, input } = await render();

    expect(element.querySelector(`label[for="${input.id}"]`)?.textContent).toBe('Porta');
    const help = element.querySelector(`#${input.getAttribute('aria-describedby')}`);
    expect(help?.textContent).toBe('Padrão: 5432');
    expect(input.getAttribute('aria-invalid')).toBeNull();
  });

  it('replaces the help with the error and marks the control invalid', async () => {
    const { fixture, element, input } = await render();

    fixture.componentInstance.error.set('Porta deve ser um número');
    await fixture.whenStable();

    const message = element.querySelector(`#${input.getAttribute('aria-describedby')}`);
    expect(message?.textContent).toBe('Porta deve ser um número');
    expect(element.textContent).not.toContain('Padrão: 5432');
    expect(input.getAttribute('aria-invalid')).toBe('true');
  });

  it('drops aria-describedby when there is neither help nor error', async () => {
    const { fixture, input } = await render();

    fixture.componentInstance.help.set(null);
    await fixture.whenStable();

    expect(input.getAttribute('aria-describedby')).toBeNull();
  });
});
