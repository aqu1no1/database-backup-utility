import { HttpClient, provideHttpClient } from '@angular/common/http';
import {
  EnvironmentProviders,
  Injectable,
  inject,
  isDevMode,
  makeEnvironmentProviders,
  provideAppInitializer,
} from '@angular/core';
import {
  Translation,
  TranslocoLoader,
  TranslocoService,
  provideTransloco,
} from '@jsverse/transloco';
import { DEFAULT_LANGUAGE, LANGUAGES, LanguageService } from './language.service';

@Injectable({ providedIn: 'root' })
export class TranslationLoader implements TranslocoLoader {
  private readonly http = inject(HttpClient);

  getTranslation(language: string) {
    return this.http.get<Translation>(`i18n/${language}.json`);
  }
}

export function provideI18n(): EnvironmentProviders {
  return makeEnvironmentProviders([
    provideHttpClient(),
    provideTransloco({
      config: {
        availableLangs: [...LANGUAGES],
        defaultLang: DEFAULT_LANGUAGE,
        fallbackLang: DEFAULT_LANGUAGE,
        missingHandler: { useFallbackTranslation: true },
        reRenderOnLangChange: true,
        prodMode: !isDevMode(),
      },
      loader: TranslationLoader,
    }),
    provideAppInitializer(() => {
      const language = inject(LanguageService).language();
      return inject(TranslocoService).load(language);
    }),
  ]);
}
