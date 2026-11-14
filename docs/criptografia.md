# Compressão e criptografia de backups

O `backup` pode comprimir o artefato com [zstd](https://facebook.github.io/zstd/) e criptografá-lo com
[age](https://age-encryption.org/v1), dois formatos abertos e estabelecidos. Nada aqui é algoritmo
próprio: o utilitário usa as crates `zstd` (binding da libzstd) e `age` (implementação de referência em
Rust), e o arquivo gerado abre com as ferramentas oficiais `zstd` e `age`/`rage`, sem este utilitário.

As duas etapas são opcionais e independentes:

```bash
dbu key generate -o ~/.config/dbu/backup.key           # imprime a chave pública age1...
dbu backup -e mysql -d app -u backup --compress --recipient age1...
dbu verify app-20261110T120000Z.sql.zst.age --identity ~/.config/dbu/backup.key
dbu restore app-20261110T120000Z.sql.zst.age --to app_copia -u backup --identity ~/.config/dbu/backup.key
```

## Compressão por SGBD

O `--compress` só aplica zstd quando o formato do SGBD ainda não é comprimido. Comprimir de novo um
artefato comprimido gasta CPU e quase não reduz o tamanho.

| SGBD | Formato do backup | Já é comprimido? | `--compress` |
| --- | --- | --- | --- |
| PostgreSQL | `pg_dump --format=custom` | sim (zlib, padrão do `pg_dump`) | ignorado, com aviso no log `info`; o arquivo continua `.dump` |
| MySQL / MariaDB | SQL do `mysqldump`/`mariadb-dump` | não | zstd nível 3 → `.sql.zst` |
| MongoDB | `mongodump --archive` (sem `--gzip`, ver [`arquitetura.md`](arquitetura.md#mongodb-formato-do-archive)) | não | zstd nível 3 → `.archive.zst` |
| SQLite | cópia da Online Backup API | não | zstd nível 3 → `.sqlite3.zst` |

A compressão vem antes da criptografia: dado criptografado não comprime.

## Criptografia com age

- `--recipient <age1...>` criptografa para uma chave pública. Repita a opção para mais de uma; qualquer
  uma das chaves privadas correspondentes abre o backup.
- `--recipients-file <ARQUIVO>` lê chaves públicas de um arquivo, uma por linha, com `#` para
  comentários. O arquivo não pode ser link simbólico nem gravável por grupo ou outros, porque quem
  troca uma chave pública passa a receber os backups.
- Só chaves X25519 nativas do age (`age1...`). Chaves SSH, plugins e senha (scrypt) não são aceitos.
- O backup fica `<nome>.age` (ou `<nome>.zst.age`) com permissão `0600`, e o manifesto passa para a
  versão 3 ([`manifesto.md`](manifesto.md#versão-3-backup-comprimido-ou-criptografado)).

O age dá confidencialidade e integridade: sem uma das chaves privadas ninguém lê o conteúdo, e um
byte alterado faz a abertura falhar. Ele **não assina** o backup: quem tem a chave pública consegue
gerar outro backup criptografado válido. Guarde o `sha256` do manifesto num lugar separado se precisar
detectar a troca do par inteiro (backup + manifesto).

## Chaves

### Gerar

```bash
dbu key generate -o ~/.config/dbu/backup.key
```

- Gera uma identidade X25519 nova e grava a chave privada no arquivo com permissão `0600`. O arquivo
  não pode existir: o comando nunca sobrescreve uma chave.
- O stdout recebe só a chave pública (com `--json`, `{"identity_file": ..., "public_keys": [...]}`).
  A chave privada nunca é impressa, registrada no log, gravada no histórico ou no manifesto.
- Não existe chave nem senha fixa no código: cada instalação gera as próprias.

### Guardar

- A chave privada não precisa ficar na máquina que faz os backups. O `backup` só usa as chaves
  públicas; a privada só é necessária no `verify` e no `restore`. Uma máquina de backup comprometida
  não consegue ler backups antigos se a chave privada não estiver nela.
- Guarde a chave privada como um segredo: gerenciador de senhas, cofre, mídia offline. O
  `--identity` exige permissão `600` ou `400` e recusa link simbólico.

### Exportar

```bash
dbu key public ~/.config/dbu/backup.key        # imprime a chave pública
```

A chave pública não é segredo e pode ir em scripts, perfis de CI ou num `--recipients-file`.

### Recuperação

**Sem nenhuma das chaves privadas, um backup criptografado não pode ser recuperado. Nem por este
utilitário, nem por quem o mantém.** Não existe porta dos fundos nem chave mestra. Para não depender de
uma só cópia:

1. Gere uma segunda chave de recuperação e guarde-a offline (por exemplo, impressa num cofre):
   `dbu key generate -o /mnt/offline/recuperacao.key`.
2. Criptografe todo backup para as duas: `--recipient <principal> --recipient <recuperação>`, ou um
   `--recipients-file` com as duas linhas.
3. Teste a recuperação periodicamente: `dbu verify <backup> --identity /mnt/offline/recuperacao.key`.

Perder a chave principal não perde os backups enquanto a de recuperação existir. Se uma chave
privada vazar, gere outra, troque o destinatário nos próximos backups e trate os backups antigos
como expostos (eles continuam abrindo com a chave vazada).

### Abrir sem este utilitário

O arquivo é age e zstd padrão:

```bash
age -d -i backup.key app-20261110T120000Z.sql.zst.age | zstd -d -o app-20261110T120000Z.sql
dbu verify app-20261110T120000Z.sql --allow-unverified
```

O `content.sha256` do manifesto v3 é o SHA-256 do arquivo aberto e serve para conferi-lo à mão.

## O que o `verify` e o `restore` conferem

Antes de abrir:

1. Origem segura (sem symlink, sem permissão aberta), como em qualquer backup.
2. Manifesto v3 válido: `content` presente, `content.file_name` sem diretório.
3. `size_bytes` e `sha256` do arquivo armazenado iguais aos do manifesto. Um arquivo alterado é
   recusado **antes** de decriptar (código 10).
4. Backup criptografado sem `--identity` → código 2; nenhuma das chaves serve → código 10.

Ao abrir:

5. O conteúdo é aberto em streaming num diretório temporário privado (`0700`, arquivo `0600`) dentro
   da pasta de dados (`<dados>/work/`), não no `/tmp`.
6. O age confere a autenticação de cada bloco; um bloco alterado falha (código 10).
7. A saída é limitada ao `content.size_bytes` do manifesto, para um arquivo forjado não encher o
   disco, e precisa bater com o `content.sha256`.

Depois, o fluxo é o mesmo de um backup sem compressão nem criptografia (`pg_restore --list`, leitura
do dump SQL ou do archive, `integrity_check`, restore). O diretório temporário é apagado no fim, com
sucesso, falha ou Ctrl+C. Se o processo morrer sem limpar, o próximo `verify`/`restore` apaga os
diretórios órfãos (cada um tem um lock que o sistema solta quando o processo termina).

Um arquivo comprimido ou criptografado sem manifesto é recusado (código 10) com a instrução de
abri-lo com `age -d`/`zstd -d` e verificar o resultado com `--allow-unverified`.

## Limites

- **Texto aberto em disco durante o backup e o restore.** O dump é gravado e verificado em claro num
  `.partial` (`0600`, com lock) antes de ser comprimido e criptografado; o `.partial` é apagado em
  seguida. No `verify`/`restore`, o conteúdo aberto fica no diretório temporário enquanto a operação
  roda. Quem tem acesso de leitura como o mesmo usuário ou root vê esse arquivo nesse intervalo. O
  `pg_restore` precisa de um arquivo pesquisável, por isso não há abertura direto para o stdin.
- **Espaço em disco:** o restore de um backup comprimido precisa do tamanho do conteúdo aberto
  (`content.size_bytes`) livre na pasta de dados.
- **Sem assinatura.** Ver "Criptografia com age" acima.
- **Perfis e modo guiado** não guardam destinatários: passe `--recipient`/`--recipients-file` em cada
  backup (o modo guiado repete as opções no comando equivalente).
