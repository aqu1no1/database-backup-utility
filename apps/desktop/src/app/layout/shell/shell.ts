import { Component } from '@angular/core';
import { RouterOutlet } from '@angular/router';
import { Sidebar } from '../sidebar/sidebar';

/**
 * Estrutura do desktop: sidebar à esquerda e a página atual à direita. Em janelas estreitas, a
 * sidebar passa para cima do conteúdo.
 */
@Component({
  selector: 'app-shell',
  imports: [RouterOutlet, Sidebar],
  templateUrl: './shell.html',
})
export class Shell {}
