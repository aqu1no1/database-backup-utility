import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Segmented, SegmentedOption } from './segmented';

type Tls = 'auto' | 'disable' | 'verify-full';

@Component({
  imports: [Segmented],
  template: `<app-segmented label="TLS" [options]="options" [(value)]="tls" />`,
})
class Host {
  readonly tls = signal<Tls>('auto');
  readonly options: readonly SegmentedOption<Tls>[] = [
    { value: 'auto', label: 'Automático', hint: 'Sem TLS só em localhost.' },
    { value: 'disable', label: 'Desativado' },
    { value: 'verify-full', label: 'Verificado', hint: 'TLS com certificado validado.' },
  ];
}

describe('Segmented', () => {
  async function render() {
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;
    const buttons = [...element.querySelectorAll('button')];
    return { fixture, element, buttons };
  }

  it('presses only the selected option and describes it with its hint', async () => {
    const { element, buttons } = await render();
    const group = element.querySelector('[role="group"]') as HTMLElement;

    expect(group.getAttribute('aria-label')).toBe('TLS');
    expect(buttons.map((button) => button.getAttribute('aria-pressed'))).toEqual([
      'true',
      'false',
      'false',
    ]);
    expect(element.querySelector(`#${group.getAttribute('aria-describedby')}`)?.textContent).toBe(
      'Sem TLS só em localhost.',
    );
  });

  it('updates the bound value and the hint when another option is clicked', async () => {
    const { fixture, element, buttons } = await render();

    buttons[2].click();
    await fixture.whenStable();

    expect(fixture.componentInstance.tls()).toBe('verify-full');
    expect(buttons[2].getAttribute('aria-pressed')).toBe('true');
    expect(element.textContent).toContain('TLS com certificado validado.');
  });

  it('shows no hint for an option without one', async () => {
    const { fixture, element, buttons } = await render();

    buttons[1].click();
    await fixture.whenStable();

    expect(element.querySelector('[role="group"]')?.getAttribute('aria-describedby')).toBeNull();
    expect(element.textContent).not.toContain('localhost');
  });
});
