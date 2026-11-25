import { TestBed } from '@angular/core/testing';
import { provideBackend } from '../../core/backend/backend.providers';
import { MockBackend } from '../../core/backend/mock-backend';
import { provideI18nTesting } from '../../core/i18n/i18n.testing';
import { HistoryPage } from './history-page';

describe('HistoryPage', () => {
  async function render(prepare: (backend: MockBackend) => void = () => undefined) {
    localStorage.clear();
    TestBed.configureTestingModule({ providers: [provideI18nTesting(), provideBackend()] });
    const backend = TestBed.inject(MockBackend);
    backend.latencyMs = 0;
    prepare(backend);
    const fixture = TestBed.createComponent(HistoryPage);
    await fixture.whenStable();
    return { fixture, backend, element: fixture.nativeElement as HTMLElement };
  }

  function button(element: HTMLElement, label: string): HTMLButtonElement {
    const found = [...element.querySelectorAll('button')].find(
      (candidate) => candidate.textContent?.trim() === label,
    );
    if (!found) {
      throw new Error(`No button ${label}`);
    }
    return found;
  }

  it('lists every operation and filters by kind', async () => {
    const { fixture, element } = await render();
    expect(element.querySelectorAll('tbody tr')).toHaveLength(7);

    button(element, 'Restaurações').click();
    await fixture.whenStable();

    expect(element.querySelectorAll('tbody tr')).toHaveLength(1);
    expect(element.querySelector('tbody')?.textContent).toContain('rst_01JC7R0A2B');
  });

  it('shows the empty state', async () => {
    const { element } = await render((backend) => (backend.history = []));

    expect(element.textContent).toContain('Nenhuma operação por aqui.');
    expect(element.querySelector('table')).toBeNull();
  });

  it('shows a read error and recovers on retry', async () => {
    const { fixture, backend, element } = await render((mock) =>
      mock.failWith('listHistory', 'unreachable'),
    );
    expect(element.querySelector('[role="alert"]')).not.toBeNull();

    backend.recover('listHistory');
    button(element, 'Tentar de novo').click();
    await fixture.whenStable();

    expect(element.querySelectorAll('tbody tr')).toHaveLength(7);
  });
});
