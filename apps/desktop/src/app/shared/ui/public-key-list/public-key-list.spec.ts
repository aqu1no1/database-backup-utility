import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { PublicKeyList } from './public-key-list';

const KEY = 'age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p';
const OTHER = 'age1lggyhqrw2nlhcxprm67z43rta597azn8gknawjehu9d9dl0jq3yqqvfafg';

@Component({
  imports: [PublicKeyList],
  template: `
    <app-public-key-list
      [(keys)]="keys"
      inputLabel="Nova chave pública"
      addLabel="Adicionar"
      removeLabel="Remover"
      invalidMessage="A chave pública age começa com age1."
      duplicateMessage="Essa chave já está na lista."
    />
  `,
})
class Host {
  readonly keys = signal<readonly string[]>([KEY]);
}

describe('PublicKeyList', () => {
  async function render() {
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;
    const input = element.querySelector('input') as HTMLInputElement;
    const addButton = [...element.querySelectorAll('button')].find(
      (button) => button.textContent?.trim() === 'Adicionar',
    ) as HTMLButtonElement;
    async function type(value: string) {
      input.value = value;
      input.dispatchEvent(new Event('input'));
      addButton.click();
      await fixture.whenStable();
    }
    return { fixture, element, input, type };
  }

  it('shows each key shortened, with the full key in the title', async () => {
    const { element } = await render();
    const item = element.querySelector('li span') as HTMLElement;

    expect(item.textContent).toBe('age1ql3z7hjy…mcac8p');
    expect(item.title).toBe(KEY);
  });

  it('adds a valid key and clears the input', async () => {
    const { fixture, input, type } = await render();

    await type(`  ${OTHER} `);

    expect(fixture.componentInstance.keys()).toEqual([KEY, OTHER]);
    expect(input.value).toBe('');
  });

  it('refuses an invalid key and explains why, without changing the list', async () => {
    const { fixture, element, input, type } = await render();

    await type('AGE-SECRET-KEY-1ABC');

    expect(fixture.componentInstance.keys()).toEqual([KEY]);
    expect(element.querySelector('[role="alert"]')?.textContent).toContain('começa com age1');
    expect(input.getAttribute('aria-invalid')).toBe('true');
  });

  it('refuses a key that is already in the list', async () => {
    const { fixture, element, type } = await render();

    await type(KEY);

    expect(fixture.componentInstance.keys()).toEqual([KEY]);
    expect(element.querySelector('[role="alert"]')?.textContent).toContain('já está na lista');
  });

  it('removes a key with an accessible label naming it', async () => {
    const { fixture, element } = await render();
    const remove = element.querySelector('li button') as HTMLButtonElement;

    expect(remove.getAttribute('aria-label')).toBe('Remover age1ql3z7hjy…mcac8p');
    remove.click();
    await fixture.whenStable();

    expect(fixture.componentInstance.keys()).toEqual([]);
  });
});
