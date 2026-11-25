import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideRouter, Router } from '@angular/router';
import { provideBackend } from '../../core/backend/backend.providers';
import { MockBackend } from '../../core/backend/mock-backend';
import { provideI18nTesting } from '../../core/i18n/i18n.testing';
import { Shell } from './shell';

@Component({ template: '' })
class Blank {}

describe('Shell', () => {
  beforeEach(() => localStorage.clear());

  async function renderAt(url: string, prepare: (backend: MockBackend) => void = () => undefined) {
    TestBed.configureTestingModule({
      imports: [Shell],
      providers: [
        provideBackend(),
        provideI18nTesting(),
        provideRouter([
          { path: 'dashboard', component: Blank },
          { path: 'backups', component: Blank },
          { path: 'settings', component: Blank },
        ]),
      ],
    });
    const backend = TestBed.inject(MockBackend);
    backend.latencyMs = 0;
    prepare(backend);
    const fixture = TestBed.createComponent(Shell);
    await TestBed.inject(Router).navigateByUrl(url);
    await fixture.whenStable();
    return fixture.nativeElement as HTMLElement;
  }

  function current(element: HTMLElement): string[] {
    return [...element.querySelectorAll('a[aria-current="page"]')].map(
      (link) => link.textContent?.trim() ?? '',
    );
  }

  it('renders the sidebar navigation next to the routed page', async () => {
    const element = await renderAt('/dashboard');
    const labels = [...element.querySelectorAll('nav a')].map((link) =>
      link.querySelector('span')?.textContent?.trim(),
    );

    expect(element.querySelector('main router-outlet')).not.toBeNull();
    expect(labels).toEqual([
      'Painel',
      'Conexões',
      'Backups',
      'Restaurações',
      'Agendamentos',
      'Histórico',
    ]);
  });

  it('marks only the link of the current page', async () => {
    expect(current(await renderAt('/backups'))).toEqual(['Backups']);
  });

  it('marks the settings link at the bottom when settings is open', async () => {
    expect(current(await renderAt('/settings'))).toEqual(['Configurações']);
  });

  it('flags schedules as planned for a later version', async () => {
    const element = await renderAt('/dashboard');
    const link = element.querySelector('a[href="/schedules"]') as HTMLElement;

    expect(link.querySelector('.sr-only')?.textContent).toBe('(Planejado · v0.5)');
    expect(link.querySelector('span[title]')?.getAttribute('aria-hidden')).toBe('true');
  });

  it('shows the first connection as the active one', async () => {
    const element = await renderAt('/dashboard');

    expect(element.querySelector('app-sidebar')?.textContent).toContain('Conexão ativa');
    expect(element.querySelector('app-sidebar [title="development"]')).not.toBeNull();
  });

  it('hides the active connection when there is none', async () => {
    const element = await renderAt('/dashboard', (backend) => (backend.connections = []));

    expect(element.querySelector('app-sidebar')?.textContent).not.toContain('Conexão ativa');
  });

  it('keeps the navigation when the connections cannot be read', async () => {
    const element = await renderAt('/dashboard', (backend) =>
      backend.failWith('listConnections', 'unreachable'),
    );

    expect(element.querySelectorAll('nav a')).toHaveLength(6);
    expect(element.querySelector('app-sidebar')?.textContent).not.toContain('Conexão ativa');
  });
});
