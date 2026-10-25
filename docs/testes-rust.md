# Política de testes — Rust (`rustc` + Cargo)

Regras para agentes de IA e pessoas que escrevem ou mudam testes neste workspace. O objetivo é ter
testes que detectem defeitos reais e regressões, sem inflar a suíte com verificações triviais,
redundantes ou frágeis.

**Qualidade vale mais que quantidade.** Cinco testes relevantes que pegam defeitos reais valem mais
que cinquenta que só confirmam detalhes triviais.

O resumo operacional está na seção 8 do [`AGENTS.md`](../AGENTS.md). As ferramentas e os comandos
de cada nível estão em [`testes.md`](testes.md).

## 1. Princípios obrigatórios

1. **Entenda o comportamento antes de escrever o teste.** Leia o card, o código, a documentação e a
   suíte existente.
2. **Cada teste precisa detectar uma falha plausível.** Se a única resposta a "o que quebraria este
   teste?" for "uma mudança de implementação", reavalie.
3. **Teste contratos, resultados e invariantes**, não detalhes internos nem contagem de chamadas
   sem significado contratual.
4. **Não existe teste por obrigação.** Nenhuma função, módulo, struct ou crate precisa de um teste
   só por existir.
5. **Compilar não é validar comportamento.** O `rustc` pega erros de tipo e de empréstimo; os testes
   verificam regras e resultados que o compilador não garante.
6. **Use a camada mais simples capaz de detectar o risco:** unitário, integração ou CLI/E2E.
7. **Não faça mock do que você quer provar.** Banco, subprocessos (`pg_dump`/`pg_restore`),
   permissões de arquivo e formato real de arquivos exigem teste real quando o risco está neles.
8. **Testes determinísticos, independentes e isolados.** Sem depender de ordem, de diretórios
   compartilhados, do relógio real nem de rede pública.
9. **Bug corrigido ganha teste de regressão** no nível adequado, quando a falha puder ser
   reproduzida automaticamente. Quando for seguro, mostre que o teste falhava sem a correção.
10. **Não mude código de produção só para fazer passar um teste mal definido.** Revise a
    especificação e o teste antes.
11. **Não adicione crates de teste sem necessidade.** A biblioteca padrão, `tempfile`,
    `testcontainers` e `tokio` já cobrem o que o projeto precisa hoje.
12. **Nunca declare uma verificação aprovada sem executá-la.** Registre o que não rodou e por quê.

## 2. Onde ficam os testes neste projeto

```text
crates/core/src/*.rs                 # unitários em #[cfg(test)] mod tests (config, Password, segredos)
crates/storage/src/lib.rs            # unitários: .partial, permissões, manifesto, checksum
crates/history/src/lib.rs            # unitários com SQLite em diretório temporário (migrations, consultas)
crates/database-adapters/src/        # unitários de parsing e escape (versões, .pgpass, dbname)
crates/database-adapters/tests/      # integração com PostgreSQL real (Testcontainers)
  support/mod.rs                     # PostgresTestDb, MysqlTestDb, MongoTestDb + fixtures (seed.sql, e2e.sql, seed.js)
  mysql_connection.rs, mysql_tools.rs # MySQL real (Testcontainers mysql:8.4)
  mongo_connection.rs, mongo_tools.rs # MongoDB real (Testcontainers mongo:7.0, standalone e replica set)
  sqlite_backup.rs                   # Backup API do SQLite com arquivos temporários (sem Docker)
apps/cli/tests/                      # contrato da CLI executando o binário real
  cli.rs                             # sem Docker: argumentos, ajuda, JSON, fontes de senha
  backup.rs, restore.rs, test_connection.rs
  sqlite.rs                          # backup/verify/restore SQLite pela CLI (sem Docker)
  mysql.rs                           # backup/verify/restore MySQL pela CLI (Testcontainers)
  mongo.rs                           # backup/verify/restore MongoDB pela CLI (Testcontainers)
  e2e.rs                             # ciclos completos com dois servidores e os dois runners
  common/mod.rs                      # Workspace (DBU_DATA_DIR, TMPDIR e saída isolados)
tests/fixtures/postgres/, mysql/, mongodb/ # dados de teste compartilhados, sem dados reais
```

- **Unitários** ficam ao lado do código, em `#[cfg(test)] mod tests`, com `#[test]` e asserts nativos.
- **Integração** fica em `tests/` da crate responsável e usa a API pública com dependências reais.
- **CLI/E2E** executa o binário e confere exit code, stdout/stderr e efeitos (arquivos, banco, histórico).
- Testes entre crates só para fronteiras relevantes. Não reestruture pastas só para seguir este guia.
- **Sem doctests.** O `AGENTS.md` proíbe comentários no código, inclusive `///`, então exemplos de
  uso ficam em `docs/` e nos próprios testes. As crates não são publicadas para terceiros. Se isso
  mudar, rever esta regra.

## 3. Escolha do nível

| Nível | O que protege | Exemplo neste projeto | O que evitar |
| --- | --- | --- | --- |
| Unitário | Regras e transformações isoladas | `read_password_file` recusa `0644`; `dbname_conninfo` escapa aspas; manifesto sem versão é recusado | Testar getter que só devolve um campo |
| Integração | Contratos com adaptadores reais | `pg_dump`/`pg_restore` com Docker e nativo; migrations do SQLite; restore sem dados parciais | Simular o banco num teste que diz verificar backup real |
| CLI / E2E | Comportamento observável do executável | Exit codes por tipo de falha; senha fora da saída; backup → restore idêntico em outro servidor | Só verificar que o binário inicia |

Suba de nível só quando a integração real fizer parte do defeito procurado. O mesmo recurso pode
ter teste unitário e de integração se cada um pega **falhas diferentes**. Não repita no E2E todos
os cenários já cobertos por unitários.

**Prioridade**, ajustada pelo risco de cada mudança:
1. Integridade dos dados de backup e restore.
2. Segurança: credenciais, permissões, TLS, injeção.
3. Persistência e transações (histórico, `--single-transaction`).
4. Tratamento de erros e falhas (exit codes, cancelamento, arquivos parciais).
5. Integrações externas e seus contratos (`pg_dump`, Docker, libpq).
6. Casos extremos e validação de entradas.
7. Fluxos principais do usuário.

## 4. Filtro de relevância (antes de cada teste)

1. **Comportamento:** que resultado ou invariante precisa continuar verdadeiro?
2. **Falha plausível:** que implementação errada faria este teste falhar?
3. **Impacto:** que prejuízo a regressão traria ao usuário ou aos dados?
4. **Nível:** qual é o teste mais simples e confiável que detecta isso?
5. **Duplicação:** algum teste existente já dá a mesma proteção?
6. **Determinismo:** ele roda repetidamente, inclusive na CI, sem instabilidade?

Sem comportamento útil, risco plausível ou valor adicional, **não escreva o teste**. Se o risco
exige infraestrutura indisponível, registre a lacuna. Não use mock como prova de integração real.

| Cenário | Decisão | Por quê |
| --- | --- | --- |
| Arquivo de senha com permissão aberta é recusado sem vazar o conteúdo | **Unitário + CLI** | Segurança; a CLI confere a mensagem e o exit code |
| Nome de banco `dbname=postgres` não redireciona o restore | **E2E** | O defeito só aparece com o libpq real |
| Ctrl+C no meio do dump não deixa `.partial` nem container | **Integração/E2E** | Integridade e limpeza dependem de processo e Docker reais |
| Senha nunca aparece em stdout, stderr, logs ou histórico | **CLI/E2E** | Contrato observável de segurança |
| `struct` pode ser instanciada | **Evitar** | O compilador já garante |
| Enum tem N variantes | **Evitar** | Detalhe estrutural, não contrato |
| Função interna chamada exatamente duas vezes | **Evitar** | Frágil, a menos que a contagem seja contrato |
| Mock de `pg_dump` "deu certo", então o backup funciona | **Inválido** | Não testa a integração verdadeira |

### O que evitar

- Getters, setters e métodos que só existem.
- Testes que repetem a implementação.
- Duplicados sem cobertura adicional.
- Testes de bibliotecas externas.
- Detalhes internos sem justificativa.
- Mocks que eliminam o comportamento que deveria ser validado.
- Snapshots extensos sem propósito.
- Testes criados só para subir métricas de cobertura.

## 5. Regras específicas de Rust neste projeto

### Cargo e compilação

- Use os comandos Cargo do workspace, nunca `rustc` direto.
- Não escreva testes cujo único propósito seja provar que o código compila.
- Não use `#[should_panic]` para fluxos que devem retornar `Err`. Verifique a variante do erro e a mensagem.
- `unwrap()`/`expect()` são proibidos em produção pelo Clippy (`[workspace.lints.clippy]`). Em
  testes são permitidos para montar fixtures e explicitar falhas inesperadas. Os binários de
  integração liberam o lint com `#![allow(clippy::unwrap_used, clippy::expect_used)]` na raiz.
- Nada de `#[ignore]` sem motivo registrado, assert vazio ou ramo que sempre passa.

### Assíncrono, processos e recursos

- Use `#[tokio::test]` para código assíncrono. Não crie runtime só para testar código síncrono.
- Isolamento: cada teste usa `tempfile` próprio. O `Workspace` dos testes da CLI define
  `DBU_DATA_DIR`, `TMPDIR` e o diretório de saída exclusivos, e remove `DBU_PG_PASSWORD*` e
  `RUST_LOG` do ambiente herdado.
- Banco: sempre `PostgresTestDb` (Testcontainers, senha aleatória, porta dinâmica). Nunca o
  PostgreSQL do Compose de desenvolvimento nem dados reais.
- Ferramentas externas: confira exit code, erro e integridade. `pg_dump` nativo ausente é
  pré-requisito não atendido, não aprovação. Com `DBU_REQUIRE_NATIVE=1` (CI), a ausência falha o teste.
- Portas: sempre dinâmicas, inclusive para "porta fechada" (bind em `:0` e liberar).

### Tempo e `sleep`

- **Evite `sleep` arbitrário.** Prefira sincronização explícita ou condição observável.
- **Exceção aceita: polling com prazo.** Laço curto (ex.: 20 ms) esperando uma condição observável
  (`.partial` apareceu, sessão encerrou, processo terminou), sempre com prazo máximo e erro claro
  se o prazo estourar.
- Espera fixa "para ver se algo acontece depois" só quando o defeito procurado é justamente um
  efeito atrasado e não há sinal observável. Nesse caso, a duração tem que estar justificada no
  teste ou no PR.

### Paralelismo

- Os testes rodam em paralelo por padrão. Não use `--test-threads=1` para esconder falha de isolamento.
- **Exceção documentada:** os cenários de `apps/cli/tests/e2e.rs` rodam em série dentro do arquivo
  (mutex `SERIAL`). Cada cenário confere, ao final, que nenhum container `dbu-pg-*` ficou para trás.
  Em paralelo, essa checagem veria containers legítimos de outro cenário. A serialização é
  necessária para a asserção, não esconde falha de isolamento.

## 6. Fluxo obrigatório do agente

**A. Entender e selecionar**
1. Ler o card, o código, os docs e os testes existentes.
2. Identificar comportamentos, riscos e invariantes, e classificar o risco (alto, médio, baixo).
3. Escolher só os cenários que trazem confiança adicional. Não existe meta de quantidade.

**B. Planejar cada cenário**
- **Dado / Quando / Então**;
- regressão que ele detecta;
- nível;
- recursos (fixtures, containers, arquivos), isolamento e limpeza.

**C. Implementar**
1. Com TDD: o teste falha **pelo motivo esperado** antes da implementação.
2. Asserts sobre resultados, efeitos e mensagens contratuais.
3. Helpers só quando reduzem duplicação real.
4. Fixture mínima, sem depender de estado de outra execução.

**D. Validar e revisar**
1. Rodar primeiro o teste ou a crate afetada, depois a verificação completa da seção 8 do `AGENTS.md`.
2. Confirmar que o teste detecta a regressão pretendida. Para regras de segurança e integridade,
   quebrar a regra temporariamente e ver o teste falhar (mutação manual), depois restaurar.
3. Revisar testes redundantes, frágeis ou lentos sem ganho de confiança.
4. Informar o que passou, o que falhou, o que não rodou e por quê.

## 7. Comandos

Os mesmos da verificação obrigatória do `AGENTS.md` (o Clippy já compila todos os targets, então
`cargo check` não é necessário):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace                                       # integração e E2E exigem Docker
```

Execução direcionada:

```bash
cargo test -p database-backup-storage                        # uma crate
cargo test -p database-backup-cli --test e2e                 # um arquivo de integração
cargo test -p database-backup-cli --test e2e hostile         # filtro pelo nome
DBU_REQUIRE_NATIVE=1 cargo test -p database-backup-cli --test e2e   # exige pg_dump 17 nativo
cargo test -p database-backup-cli --test e2e -- --nocapture  # ver a saída ao diagnosticar
cargo test --workspace --no-run                              # só compilar os testes
```

Logs de diagnóstico do E2E: `target/e2e-logs/<cenário>/` (ou `DBU_E2E_LOG_DIR`), gravados só quando
um cenário falha.

## 8. Checklist para encerrar

- [ ] Entendi o comportamento e li os testes existentes antes de criar novos.
- [ ] Cada teste protege um comportamento, contrato ou invariante relevante.
- [ ] Sei qual falha plausível faz cada teste falhar.
- [ ] Nada trivial, redundante ou criado só para cobertura.
- [ ] Não confundi resultado de mock com integração real.
- [ ] Estado independente e limpeza de recursos garantidos (containers, temporários, `.partial`).
- [ ] Erros, cancelamento e concorrência testados na medida do risco.
- [ ] Nenhum acesso a banco, arquivo ou rede reais fora dos recursos de teste.
- [ ] `fmt`, `clippy`, `build` e `test` executados, com os resultados registrados.

## Referências

- [The Rust Programming Language — Writing Automated Tests](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [Cargo Book — Tests](https://doc.rust-lang.org/cargo/guide/tests.html)
- [Cargo Book — `cargo test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html)
- [Clippy — Usage](https://doc.rust-lang.org/stable/clippy/usage.html)
