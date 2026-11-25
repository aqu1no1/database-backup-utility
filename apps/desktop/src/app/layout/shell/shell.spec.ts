import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideRouter, Router } from '@angular/router';
import { provideI18nTesting } from '../../core/i18n/i18n.testing';
import { Shell } from './shell';

@Component({ template: '' })
class Blank {}

describe('Shell', () => {
  beforeEach(() => localStorage.clear());

  async function renderAt(url: string) {
    TestBed.configureTestingModule({
      imports: [Shell],
      providers: [
        provideI18nTesting(),
        provideRouter([
          { path: 'dashboard', component: Blank },
          { path: 'backups', component: Blank },
          { path: 'settings', component: Blank },
        ]),
      ],
    });
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
});
