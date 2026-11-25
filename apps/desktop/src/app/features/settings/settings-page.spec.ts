import { TestBed } from '@angular/core/testing';
import { provideBackend } from '../../core/backend/backend.providers';
import { MockBackend } from '../../core/backend/mock-backend';
import { provideI18nTesting } from '../../core/i18n/i18n.testing';
import { SettingsPage } from './settings-page';

describe('SettingsPage', () => {
  async function render(prepare: (backend: MockBackend) => void = () => undefined) {
    localStorage.clear();
    TestBed.configureTestingModule({ providers: [provideI18nTesting(), provideBackend()] });
    const backend = TestBed.inject(MockBackend);
    backend.latencyMs = 0;
    prepare(backend);
    const fixture = TestBed.createComponent(SettingsPage);
    await fixture.whenStable();
    return { fixture, element: fixture.nativeElement as HTMLElement };
  }

  async function press(
    fixture: { whenStable(): Promise<unknown> },
    element: HTMLElement,
    label: string,
  ) {
    const button = [...element.querySelectorAll('button')].find(
      (candidate) => candidate.textContent?.trim() === label,
    );
    button?.click();
    await fixture.whenStable();
  }

  it('changes the language and the theme at once', async () => {
    const { fixture, element } = await render();

    await press(fixture, element, 'EN-US');
    expect(element.querySelector('h1')?.textContent).toBe('Settings');

    await press(fixture, element, 'Light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
  });

  it('saves the tool runner', async () => {
    const { fixture, element } = await render();

    await press(fixture, element, 'Docker');

    const docker = [...element.querySelectorAll('button')].find(
      (candidate) => candidate.textContent?.trim() === 'Docker',
    );
    expect(docker?.getAttribute('aria-pressed')).toBe('true');
  });

  it('keeps the previous value and warns when saving fails', async () => {
    const { fixture, element } = await render((backend) =>
      backend.failWith('saveSettings', 'unreachable'),
    );

    await press(fixture, element, 'Docker');

    expect(element.querySelector('[role="alert"]')?.textContent).toContain(
      'Não foi possível salvar',
    );
  });

  it('shows a read error', async () => {
    const { element } = await render((backend) => backend.failWith('getSettings', 'unreachable'));

    expect(element.querySelector('[role="alert"]')).not.toBeNull();
  });
});
