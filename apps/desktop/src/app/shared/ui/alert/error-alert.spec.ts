import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';
import { AppError, ERROR_CODES } from '../../../core/backend/backend.models';
import { provideI18nTesting } from '../../../core/i18n/i18n.testing';
import { ErrorAlert } from './error-alert';

describe('ErrorAlert', () => {
  async function render(language: string, error: unknown) {
    localStorage.clear();
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    TestBed.inject(TranslocoService).setActiveLang(language);
    const fixture = TestBed.createComponent(ErrorAlert);
    fixture.componentRef.setInput('title', 'Title');
    fixture.componentRef.setInput('error', error);
    await fixture.whenStable();
    return fixture.nativeElement as HTMLElement;
  }

  const cases = ['pt-BR', 'en-US'].flatMap((language) =>
    ERROR_CODES.map((code) => [language, code] as const),
  );

  it.each(cases)('translates in %s the code %s', async (language, code) => {
    const element = await render(language, new AppError(code));
    const translations = TestBed.inject(TranslocoService).getTranslation(language);

    expect(translations[`errors.${code}.title`]).toBeTruthy();
    expect(element.textContent).toContain(translations[`errors.${code}.title`]);
    expect(element.textContent).toContain(translations[`errors.${code}.hint`]);
    expect(element.querySelector('.font-mono')?.textContent).toContain(code);
  });

  it('hides the text of an unexpected error', async () => {
    const element = await render('pt-BR', new Error('secret detail'));

    expect(element.textContent).toContain('Erro inesperado');
    expect(element.textContent).not.toContain('secret detail');
    expect(element.querySelector('.font-mono')).toBeNull();
  });
});
