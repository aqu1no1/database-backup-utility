import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';
import enUS from '../../../../public/i18n/en-US.json';
import ptBR from '../../../../public/i18n/pt-BR.json';
import { provideI18nTesting } from './i18n.testing';
import { LanguageService } from './language.service';

function keys(value: object, prefix = ''): string[] {
  return Object.entries(value).flatMap(([key, child]) =>
    typeof child === 'object' && child !== null
      ? keys(child, `${prefix}${key}.`)
      : [`${prefix}${key}`],
  );
}

describe('LanguageService', () => {
  beforeEach(() => {
    localStorage.clear();
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
  });

  it('starts in pt-BR when nothing is stored', () => {
    const service = TestBed.inject(LanguageService);
    TestBed.tick();

    expect(service.language()).toBe('pt-BR');
    expect(document.documentElement.lang).toBe('pt-BR');
  });

  it('ignores an unknown stored language', () => {
    localStorage.setItem('dbu-lang', 'fr-FR');

    expect(TestBed.inject(LanguageService).language()).toBe('pt-BR');
  });

  it('restores, applies and persists the language', () => {
    localStorage.setItem('dbu-lang', 'en-US');
    const service = TestBed.inject(LanguageService);
    const transloco = TestBed.inject(TranslocoService);
    TestBed.tick();

    expect(transloco.translate('nav.dashboard')).toBe('Dashboard');

    service.set('pt-BR');
    TestBed.tick();

    expect(transloco.translate('nav.dashboard')).toBe('Painel');
    expect(localStorage.getItem('dbu-lang')).toBe('pt-BR');
  });

  it('has the same keys in both languages', () => {
    expect(keys(enUS).sort()).toEqual(keys(ptBR).sort());
  });
});
