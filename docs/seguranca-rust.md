# Política de segurança — Rust, rustc e Cargo

Regras para agentes de IA e pessoas que analisam, implementam, revisam ou mantêm este workspace.
Este documento diz **como** decidir e agir. O estado atual dos controles, com as evidências, as
exceções ativas e os riscos aceitos, está em [`seguranca.md`](seguranca.md).

Resumo operacional na seção 4 do [`AGENTS.md`](../AGENTS.md). Política de testes em
[`testes-rust.md`](testes-rust.md).

## Objetivo

Proteger dados, recursos, credenciais e usuários reduzindo vulnerabilidades reais e regressões de
segurança. A segurança aqui é **orientada a risco e a evidência**, não à quantidade de checagens,
bibliotecas ou relatórios.

Não fazer:
- dizer que o código é seguro porque compila ou porque é Rust;
- adicionar medida complexa sem risco plausível que a justifique;
- tratar o resultado de ferramenta (Clippy, cargo-deny) como prova de ausência de vulnerabilidade;
- corrigir uma suspeita sem entender o comportamento afetado;
- declarar uma verificação aprovada sem tê-la executado.

**Princípio:** segurança real, verificável e proporcional ao risco, com a menor complexidade necessária.

## 1. Análise antes de mudar código

1. Entender o comportamento esperado e as regras do domínio (backup, restore, verify, credenciais).
2. Localizar entrada, processamento, persistência e saída dos dados.
3. Identificar quem controla cada entrada: usuário da CLI, servidor PostgreSQL, arquivos de backup
   e manifesto, variáveis de ambiente, processos externos (`pg_dump`, `pg_restore`, `docker`) e dependências.
4. Identificar os ativos: senhas, dados dos bancos, arquivos de backup, histórico SQLite, disponibilidade.
5. Identificar as fronteiras de confiança e as permissões de cada componente.
6. Pensar em como entradas inválidas ou maliciosas poderiam causar dano.
7. Conferir as proteções e os testes que já existem (em `seguranca.md`).
8. Ver se a mudança mexe em dependências, permissões, operações de sistema ou comportamento de segurança.
9. Classificar o risco e escolher verificações proporcionais.
10. Planejar a correção mínima que preserve a funcionalidade.

Em tarefas grandes, registrar o plano antes de codificar.

### 1.1 Prioridade por risco

| Nível | Exemplos neste projeto | Tratamento |
| --- | --- | --- |
| **Crítico** | Execução de comando injetado; senha exposta; restore sobrescrevendo banco errado | Parar a operação afetada, corrigir com prioridade máxima, teste de regressão obrigatório |
| **Alto** | Backup corrompido aceito; origem de restore manipulável; permissões abertas; advisory explorável no binário | Corrigir antes da entrega, com teste de regressão |
| **Médio** | Validação incompleta; limite de recurso ausente; mensagem com dado não crítico | Corrigir conforme exposição, documentando |
| **Baixo** | Endurecimento sem vetor relevante no contexto atual | Evitar mudança desproporcional; registrar se fizer sentido |

A classificação usa **impacto + explorabilidade + exposição**, e não só o nome da categoria.

## 2. Quando uma medida é relevante

Uma medida ou teste de segurança deve:
- tratar ameaça plausível e proteger um ativo, contrato, fronteira ou operação sensível;
- ter efeito explicável e, quando possível, verificável;
- ser a opção mais simples com proteção equivalente;
- não depender de mock que esconda o risco;
- continuar válida depois de refatorações que preservem o contrato.

**Filtro:** se não dá para explicar *o que pode dar errado, por que importa e como a medida reduz o
risco*, a proposta deve ser reconsiderada.

## 3. Regras por área

### 3.1 Entradas e validação

- Considere não confiáveis os argumentos da CLI, as variáveis de ambiente, os arquivos de backup e de
  manifesto, as respostas do PostgreSQL e a saída de processos externos.
- Valide formato, tamanho, limites e permissões **antes** de qualquer efeito colateral. Exemplos no
  projeto: arquivo de senha até 4 KiB e `0600`; manifesto com `manifest_version` suportada antes do hash.
- Use tipos e enums validados (`PostgresConnectionConfig`, `SecretSource`) em vez de strings soltas.
- Validação sintática não é autenticação nem regra de negócio.
- Não remova dados em silêncio só para uma validação passar.

### 3.2 Arquivos, diretórios e caminhos

- Caminhos vindos do usuário são potencialmente perigosos. Origem de restore e arquivo de senha
  recusam symlink, arquivo não regular e permissão aberta (`check_restore_source`, `read_password_file`).
- `starts_with`, remover `..` ou usar `canonicalize` não são defesa universal. Cada um tem limites e
  pode sofrer condição de corrida.
- Segredos e backups ficam em `0600`, e o diretório criado pela CLI fica em `0700`.
- Para criação exclusiva, prefira `OpenOptions::create_new(true)` a `exists()` seguido de criação.
  O backup já falha se o arquivo existir.
- Escrita importante: arquivo temporário, `fsync` e `rename` atômico (`.partial` do storage).
- Não sobrescreva nem apague sem conferir origem, destino e política de substituição.
- Arquivos temporários são limpos em falha e cancelamento (`.pgpass`, `.partial`, containers do runner).
- Permissões e caminhos do Unix não valem igual no Windows. Diferenças em `seguranca.md`.

### 3.3 Processos externos

- `std::process::Command`/`tokio::process::Command` com **argumentos separados**. Nunca `sh -c`
  ou `cmd /C` com entrada concatenada.
- `Command::arg` não chama shell, mas isso **não impede** injeção de opções nem a interpretação do
  valor pelo programa chamado. Exemplo real: um `--dbname` com `=` virava string de conexão no
  libpq. A correção foi passar sempre `dbname='<escapado>'`.
- Senha nunca em argumento de processo. Use `.pgpass` temporário via `PGPASSFILE`.
- Defina timeout, cancelamento (matar o cliente e remover o container), captura de stderr com
  limite e conferência do exit code.
- Não confie só no nome do binário no `PATH`: confira versão e compatibilidade antes de usar
  (`PgTools::detect`).

### 3.4 Segredos

- Nada de senha, token ou chave real em código, testes, logs, exemplos, commits ou mensagens de erro.
- `Password` esconde o valor no `Debug` e zera os bytes no `Drop`. Novos tipos com segredo precisam
  do mesmo cuidado. Mesmo assim, não afirme que a memória foi limpa com garantia.
- O histórico guarda só a origem da senha (`secret_ref`), nunca o valor.
- `.env` fica fora do Git. `.env.example` só tem valores fictícios.
- Se um segredo real vazar, apagar do código não basta: revogar ou rotacionar e revisar o histórico do Git.

### 3.5 Integridade e criptografia

- Não crie algoritmos próprios. Hash, criptografia, assinatura e autenticação resolvem problemas diferentes.
- O SHA-256 do manifesto detecta corrupção, mas **não autentica** (ver `manifesto.md#limites`).
  Autenticidade fica para o DB-31 (`age`), com gestão e recuperação de chaves definidas antes.
- Nunca registre material de chave nem dado descriptografado.

### 3.6 Concorrência e disponibilidade

- Analise condições de corrida em arquivos e operações de banco. Exemplo: troca de nomes do banco
  no `--overwrite`, TOCTOU entre checksum e `pg_restore` (risco aceito em `seguranca.md`).
- Falha ou cancelamento não pode deixar arquivo, processo ou transação inconsistentes
  (`--single-transaction`, remoção do banco criado, `.partial`).
- Safe Rust sem data race não significa ausência de race condition lógica.

### 3.7 Rede e banco de dados

- TLS com certificado e host verificados para hosts remotos (`verify-full`). Não desative a
  verificação como solução, nem em desenvolvimento.
- Consultas parametrizadas. Identificadores, que não podem ser parâmetros, passam por `quote_ident`.
- Timeouts de conexão e consulta.
- HTTP, SSRF, autorização de API e autenticação de usuários **não existem hoje**. Quando o desktop
  (Tauri, v0.3.0) ou a nuvem (v0.4.0) chegarem, esta seção ganha as regras correspondentes.

### 3.8 `unsafe` e FFI

- `unsafe` é **proibido no workspace**: `[workspace.lints.rust] unsafe_code = "forbid"`. Não há FFI.
- Se um dia for indispensável, num PR próprio, com justificativa de por que Safe Rust não atende:
  1. liberar só na crate afetada, nunca no workspace inteiro;
  2. bloco mínimo, com `// SAFETY:` explicando por que as precondições valem naquele ponto. Essa é
     a **única exceção** à regra do `AGENTS.md` de não escrever comentários no código;
  3. invariantes documentadas em `docs/` e testes que as exercitem; considerar Miri.

## 4. Cadeia de dependências

### 4.1 Antes de adicionar uma crate

1. Ela é mesmo necessária? Prefira `std` e dependências que o projeto já usa.
2. Avalie manutenção, procedência, histórico de advisories e features habilitadas.
3. Revise com atenção crates com `build.rs`, macros procedurais ou código nativo, que executam
   durante o build.
4. A licença precisa estar na allowlist do `deny.toml`. Se não estiver, ver a triagem (§4.4).
5. Registre no PR as decisões incomuns.

`cargo build` e `cargo test` executam `build.rs` e macros de dependências. Não compile código de
origem não confiável num ambiente com segredos.

### 4.2 Lockfile

- `Cargo.lock` versionado (workspace de aplicação). A CI e a release usam `--locked`, que falha se o
  lock precisar mudar. Isso garante reprodutibilidade, não segurança das dependências.
- Revise mudanças no `Cargo.lock`, principalmente fontes, versões novas e dependências transitivas.
- Não rode `cargo update` geral numa correção pontual. Use `cargo update -p <crate> --precise <versão>`.

### 4.3 `cargo-deny` (`deny.toml`)

O job `Rust / Supply Chain` roda `cargo deny check` em todo PR e push na `main`. O `audit.yml`
roda `cargo deny check advisories` toda semana, para pegar advisories publicados sem PR novo.

| Check | Regra |
| --- | --- |
| advisories | Vulnerabilidade e versão yanked falham sempre, inclusive em dependência transitiva e de teste. `unmaintained` falha só em dependência direta; transitiva vira aviso e entra na triagem |
| licenses | Allowlist com as licenças em uso (MIT, Apache-2.0, ISC, BSD-3-Clause, BSL-1.0, Zlib, Unicode-3.0, Unlicense). Exceção **por crate**: `option-ext` MPL-2.0 |
| bans | Versões duplicadas só avisam; wildcard em versão é proibido |
| sources | Só crates.io; registry ou git desconhecido falham |

### 4.4 Triagem de achados

Quando o Supply Chain ou o audit semanal falhar:

1. **Ler o achado:** ID RustSec, crate, versões afetadas e corrigidas, tipo (vulnerabilidade,
   yanked, unmaintained, licença, fonte).
2. **Avaliar o alcance:**
   - `cargo tree -i <crate> -e normal` mostra se a crate entra no binário ou se é só de teste;
   - o código vulnerável é alcançável pelo nosso uso?
3. **Corrigir, nesta ordem:**
   1. atualização mínima (`cargo update -p <crate> --precise <corrigida>`);
   2. atualizar a dependência direta que a traz;
   3. trocar ou remover a dependência.

   Em todos os casos, com a verificação completa verde.
4. **Sem correção disponível:** criar uma exceção **registrada**:
   - `deny.toml`: `ignore = [{ id = "RUSTSEC-AAAA-NNNN", reason = "<por que não afeta / mitigação>" }]`;
   - `seguranca.md`, tabela "Exceções ativas": ID ou crate, motivo, alcance, condição de remoção e
     data de revisão (no máximo a próxima atualização de toolchain);
   - card no Linear para acompanhar a remoção.
5. **Licença fora da allowlist:**
   - recusar a dependência; ou
   - criar exceção **por crate** com motivo; ou
   - ampliar a allowlist num PR próprio, justificado.
6. **Proibido:**
   - remover checks ou seções do `deny.toml`;
   - `ignore` sem `reason`;
   - exceção global por categoria;
   - rebaixar severidade;
   - pular o job.
7. Um advisory novo pode fazer falhar um PR sem relação com ele. Isso é intencional: corrigir antes,
   num PR `fix(deps): …`.

### 4.5 Avisos de terceiros na distribuição

MIT, Apache e afins exigem que os avisos de copyright e licença acompanhem o binário. O job de release
gera `THIRD-PARTY-LICENSES.md` com `cargo-about` (`about.toml` + `about.hbs`, só dependências de
runtime) e coloca o arquivo no tarball. Uma dependência com licença fora do `about.toml` faz a
release falhar, e isso segue a mesma triagem do §4.4.

## 5. Toolchain e pipeline

### 5.1 Toolchain fixada

- `rust-toolchain.toml` é a **única** fonte da versão do Rust (canal exato e componentes `clippy` e
  `rustfmt`). Os workflows rodam `rustup toolchain install`, que lê o arquivo. Nenhum workflow fixa
  a versão em outro lugar.
- **Atualização deliberada**, a cada nova Rust stable (~6 semanas, card recorrente no Linear) ou
  antes se houver advisory na toolchain:
  1. branch `chore/rust-1.NN`, trocar o `channel`;
  2. verificação completa e `cargo deny check`;
  3. lints novos do Clippy são corrigidos no mesmo PR. Não desligar lint para passar; exceção
     localizada só com justificativa no PR;
  4. revisar as "Exceções ativas" de `seguranca.md`;
  5. PR `chore: update Rust to 1.NN`, nunca junto com mudança funcional.
- Avisos do compilador que indicam erro real ou quebra futura são resolvidos, não silenciados.
  Clippy aprovado é lint aprovado, não auditoria de segurança.

### 5.2 CI/CD

- Permissão mínima: `contents: read` por padrão; `contents: write` só no job de release.
- Sem `pull_request_target`. Código de PR não roda com segredos de release.
- Actions de terceiros fixadas por SHA (`docs/CI.md`).
- Dado não confiável nunca é concatenado em `run:`. Inputs entram por `env:`.
- A release só sai por `workflow_dispatch` na `main`.
- Checks obrigatórios na `main`: `Rust / Quality & Unit`, `Rust / PostgreSQL Integration`,
  `Rust / Supply Chain`.

## 6. Testes de segurança

Seguem [`testes-rust.md`](testes-rust.md). Um teste de segurança precisa falhar diante de pelo menos
uma violação plausível. Para regras de segurança, isso é confirmado com **mutação manual**: quebrar a
regra temporariamente, ver o teste falhar e restaurar sem levar a alteração para o commit. Os cenários
cobertos hoje e as mutações já feitas estão em `testes.md` e `seguranca.md`.

Evitar:
- testes que só verificam que uma biblioteca foi importada;
- mocks que substituem justamente a validação testada;
- credenciais ou destinos reais;
- assumir que ausência de `panic!` significa ausência de exploração.

## 7. Fluxo de desenvolvimento seguro

1. **Reconhecer:** código, configuração, entradas, saídas, permissões, dependências e proteções existentes.
2. **Analisar o risco:** cenário, precondição, impacto e classificação. É real, plausível ou teórico?
3. **Planejar:** a menor mudança que reduz o risco e os testes necessários.
4. **Implementar:** validação no ponto de confiança certo, privilégio mínimo, código simples.
5. **Validar:** testes do cenário, verificação completa (`AGENTS.md` §8) e `cargo deny check`;
   revisar mudanças no `Cargo.lock` e na pipeline.
6. **Revisar:**
   - a proteção cobre o risco?
   - entrou algum vazamento, regressão ou privilégio excessivo?
   - informar o que rodou, o que não rodou e os riscos residuais.

## 8. Comandos

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo test --workspace --locked
cargo deny check                       # se o cargo-deny estiver instalado; a CI sempre roda
cargo tree -i <crate> -e normal        # alcance de uma dependência no binário
```

Sem `--all-features`, porque o workspace não tem features. O `cargo check` não é necessário, porque o
Clippy já compila todos os alvos. Ferramenta indisponível ou falha de rede **deve ser relatada**,
nunca mascarada como sucesso.

## 9. Modelo de achado de segurança

```markdown
### [SEVERIDADE] Título do achado

- **Local:** arquivo e função/linha, quando conhecido.
- **Cenário:** como o problema pode ocorrer.
- **Precondições:** que acesso ou entrada é necessário.
- **Impacto:** que ativo, permissão ou garantia seria afetado.
- **Evidência:** trecho de código, teste, reprodução segura ou ferramenta.
- **Status da evidência:** confirmado, plausível ou precisa de validação.
- **Correção proposta:** menor mudança adequada.
- **Teste de regressão:** como validar o comportamento seguro.
- **Risco residual:** o que ainda não foi eliminado ou verificado.
```

Não invente arquivos, linhas, explorações, CVEs nem resultados de ferramenta. Sem comprovação, o
ponto é hipótese e o relatório diz o que falta verificar.

## 10. Checklist para PRs

- [ ] Entendi os ativos, as entradas e as fronteiras de confiança afetados.
- [ ] O cenário de risco e o impacto estão descritos.
- [ ] Validações no ponto certo; arquivos, processos e operações destrutivas avaliados.
- [ ] Nenhum segredo real em código, logs ou testes.
- [ ] Permissões mínimas.
- [ ] Dependências novas avaliadas (§4.1); `cargo deny check` verde, ou exceção registrada (§4.4).
- [ ] Nenhum `unsafe`. Se houver, a exceção do §3.8 foi seguida.
- [ ] Testes de segurança relevantes, sem duplicados triviais.
- [ ] Verificação completa executada; o que não rodou está explicado.
- [ ] `seguranca.md` atualizado se um controle, uma exceção ou um risco mudou.

## 11. Regras inegociáveis

1. Não executar ação destrutiva em ambiente real sem autorização explícita e escopo definido.
2. Não expor, registrar ou publicar credenciais reais.
3. Não ocultar falhas de segurança, de ferramentas ou de testes.
4. Não adicionar `unsafe` fora da exceção do §3.8.
5. Não confiar em entrada externa só porque passou por uma validação superficial.
6. Não aprovar uma dependência só porque o Cargo a compila.
7. Não propor correção que crie vulnerabilidade maior que a original.
8. Não dizer que a segurança está total porque testes, Clippy ou cargo-deny passaram.
9. Não mudar controles de segurança para contornar teste ou acelerar entrega sem justificativa e aprovação.
10. Segurança dos dados e do usuário acima de métricas artificiais de qualidade.

## Referências

- [Rustonomicon](https://doc.rust-lang.org/nomicon/)
- [`std::process::Command`](https://doc.rust-lang.org/std/process/struct.Command.html)
- [`std::fs::OpenOptions`](https://doc.rust-lang.org/std/fs/struct.OpenOptions.html)
- [Cargo — build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html)
- [RustSec Advisory Database](https://rustsec.org/)
- [cargo-deny](https://embarkstudios.github.io/cargo-deny/)
- [cargo-about](https://embarkstudios.github.io/cargo-about/)
- [Clippy](https://doc.rust-lang.org/clippy/)
