import { DOCUMENT } from '@angular/common';
import { Injectable, effect, inject, signal } from '@angular/core';
import { TranslocoService } from '@jsverse/transloco';

export const LANGUAGES = ['pt-BR', 'en-US'] as const;
export type Language = (typeof LANGUAGES)[number];

export const DEFAULT_LANGUAGE: Language = 'pt-BR';
const STORAGE_KEY = 'dbu-lang';

@Injectable({ providedIn: 'root' })
export class LanguageService {
  private readonly document = inject(DOCUMENT);
  private readonly transloco = inject(TranslocoService);
  readonly language = signal<Language>(this.read());

  constructor() {
    effect(() => {
      const language = this.language();
      this.transloco.setActiveLang(language);
      this.document.documentElement.lang = language;
      this.write(language);
    });
  }

  set(language: Language): void {
    this.language.set(language);
  }

  private read(): Language {
    try {
      const stored = this.document.defaultView?.localStorage.getItem(STORAGE_KEY);
      return LANGUAGES.find((language) => language === stored) ?? DEFAULT_LANGUAGE;
    } catch {
      return DEFAULT_LANGUAGE;
    }
  }

  private write(language: Language): void {
    try {
      this.document.defaultView?.localStorage.setItem(STORAGE_KEY, language);
    } catch {
      return;
    }
  }
}
