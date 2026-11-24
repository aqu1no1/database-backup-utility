import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { provideBackend } from '../../core/backend/backend.providers';
import { MockBackend } from '../../core/backend/mock-backend';
import { provideI18nTesting } from '../../core/i18n/i18n.testing';
import { LanguageService } from '../../core/i18n/language.service';
import { DashboardPage } from './dashboard-page';

describe('DashboardPage', () => {
  async function render(prepare: (backend: MockBackend) => void = () => undefined) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [provideI18nTesting(), provideBackend(), provideRouter([])],
    });
    const backend = TestBed.inject(MockBackend);
    backend.latencyMs = 0;
    prepare(backend);
    const fixture = TestBed.createComponent(DashboardPage);
    await fixture.whenStable();
    return { fixture, element: fixture.nativeElement as HTMLElement };
  }

  it('summarizes connections, running backups and recent activity', async () => {
    const { element } = await render();

    expect(element.textContent).toContain(
      'PostgreSQL 1 · MySQL 1 · MariaDB 1 · MongoDB 1 · SQLite 1',
    );
    expect(element.querySelector('section')?.textContent).toContain('bkp_01JC7T5N9P');
    expect(element.querySelectorAll('section app-operation-status')).toHaveLength(3);
  });

  it('shows the empty values', async () => {
    const { element } = await render((backend) => {
      backend.connections = [];
      backend.history = [];
    });

    expect(element.textContent).toContain('nenhum backup ainda');
    expect(element.textContent).toContain('nenhuma conexão');
  });

  it('shows a read error', async () => {
    const { element } = await render((backend) =>
      backend.failWith('listConnections', 'unreachable'),
    );

    expect(element.querySelector('[role="alert"]')).not.toBeNull();
  });

  it('changes the texts with the language', async () => {
    const { fixture, element } = await render();

    TestBed.inject(LanguageService).set('en-US');
    await fixture.whenStable();

    expect(element.querySelector('h1')?.textContent).toBe('Dashboard');
    expect(element.textContent).toContain('Recent activity');
  });
});
