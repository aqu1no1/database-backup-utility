import { EnvironmentProviders, importProvidersFrom } from '@angular/core';
import { TranslocoTestingModule } from '@jsverse/transloco';
import enUS from '../../../../public/i18n/en-US.json';
import ptBR from '../../../../public/i18n/pt-BR.json';
import { DEFAULT_LANGUAGE, LANGUAGES } from './language.service';

export function provideI18nTesting(): EnvironmentProviders {
  return importProvidersFrom(
    TranslocoTestingModule.forRoot({
      langs: { 'pt-BR': ptBR, 'en-US': enUS },
      translocoConfig: {
        availableLangs: [...LANGUAGES],
        defaultLang: DEFAULT_LANGUAGE,
        reRenderOnLangChange: true,
      },
      preloadLangs: true,
    }),
  );
}
