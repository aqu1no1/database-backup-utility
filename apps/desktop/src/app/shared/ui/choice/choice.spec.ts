import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Check } from './check';
import { OptionCard } from './option-card';

@Component({
  imports: [Check, OptionCard],
  template: `
    <app-check
      label="Comprimir (zstd)"
      [(checked)]="compress"
      [disabled]="compressDisabled()"
      reason="Não se aplica ao PostgreSQL: o formato custom do pg_dump já é comprimido."
    />
    <app-option-card
      name="destino"
      label="Sobrescrever a origem"
      [disabled]="overwriteDisabled()"
      reason="No MySQL, o restore só é feito em um banco novo."
      (choose)="chosen.set('overwrite')"
    />
  `,
})
class Host {
  readonly compress = signal(false);
  readonly compressDisabled = signal(false);
  readonly overwriteDisabled = signal(false);
  readonly chosen = signal<string | null>(null);
}

describe('Check and OptionCard', () => {
  async function render() {
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    return { fixture, element: fixture.nativeElement as HTMLElement };
  }

  it('toggles the bound value when the checkbox is clicked', async () => {
    const { fixture, element } = await render();
    const checkbox = element.querySelector('app-check input') as HTMLInputElement;

    checkbox.click();
    await fixture.whenStable();

    expect(fixture.componentInstance.compress()).toBe(true);
  });

  it('shows the reason next to a disabled checkbox and links it for screen readers', async () => {
    const { fixture, element } = await render();
    fixture.componentInstance.compressDisabled.set(true);
    await fixture.whenStable();
    const checkbox = element.querySelector('app-check input') as HTMLInputElement;

    expect(checkbox.disabled).toBe(true);
    const reason = element.querySelector(`#${checkbox.getAttribute('aria-describedby')}`);
    expect(reason?.textContent).toContain('pg_dump já é comprimido');
  });

  it('emits choose when an enabled option card is picked', async () => {
    const { fixture, element } = await render();

    (element.querySelector('app-option-card input') as HTMLInputElement).click();
    await fixture.whenStable();

    expect(fixture.componentInstance.chosen()).toBe('overwrite');
  });

  it('blocks a disabled option card and explains why', async () => {
    const { fixture, element } = await render();
    fixture.componentInstance.overwriteDisabled.set(true);
    await fixture.whenStable();
    const radio = element.querySelector('app-option-card input') as HTMLInputElement;

    expect(radio.disabled).toBe(true);
    expect(element.querySelector('app-option-card')?.textContent).toContain('banco novo');
  });
});
