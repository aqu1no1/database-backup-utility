import { TestBed } from '@angular/core/testing';
import { provideRouter, Router } from '@angular/router';
import { App } from './app';
import { provideBackend } from './core/backend/backend.providers';
import { provideI18nTesting } from './core/i18n/i18n.testing';
import { routes } from './app.routes';

describe('App', () => {
  beforeEach(() => {
    TestBed.configureTestingModule({
      imports: [App],
      providers: [provideRouter(routes), provideI18nTesting(), provideBackend()],
    });
  });

  it('opens the dashboard inside the shell on the root route', async () => {
    const fixture = TestBed.createComponent(App);
    const router = TestBed.inject(Router);
    await router.navigateByUrl('/');
    await fixture.whenStable();

    const element = fixture.nativeElement as HTMLElement;
    expect(router.url).toBe('/dashboard');
    expect(element.querySelector('app-shell main app-dashboard-page h1')?.textContent).toBe(
      'Painel',
    );
  });

  it('redirects unknown routes to the dashboard', async () => {
    const fixture = TestBed.createComponent(App);
    const router = TestBed.inject(Router);
    await router.navigateByUrl('/does-not-exist');
    await fixture.whenStable();

    expect(router.url).toBe('/dashboard');
  });
});
