import { TestBed } from '@angular/core/testing';
import { provideRouter, Router } from '@angular/router';
import { Shell } from './shell';

describe('Shell', () => {
  it('marks the dashboard link as the current page on the root route', async () => {
    TestBed.configureTestingModule({
      imports: [Shell],
      providers: [provideRouter([{ path: '', children: [] }])],
    });
    const fixture = TestBed.createComponent(Shell);
    await TestBed.inject(Router).navigateByUrl('/');
    await fixture.whenStable();

    const element = fixture.nativeElement as HTMLElement;
    const link = element.querySelector('nav a');

    expect(element.querySelector('main router-outlet')).not.toBeNull();
    expect(link?.getAttribute('aria-current')).toBe('page');
  });
});
