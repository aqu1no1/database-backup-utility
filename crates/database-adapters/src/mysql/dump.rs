use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::tools::MysqlVersion;

const KEPT_LINE_BYTES: usize = 256;
const KEPT_TAIL_BYTES: usize = 32;
const HEADER: &str = "-- MySQL dump ";
const FOOTER: &str = "-- Dump completed";
const DATABASE_STATEMENTS: [&str; 3] = ["USE ", "CREATE DATABASE", "DROP DATABASE"];
const TRIGGER_MARKER: &str = "*/ /*!50003 TRIGGER ";
const EVENT_MARKER: &str = "*/ /*!50106 EVENT ";
const VIEW_DEFINER: &str = "/*!50013 DEFINER=";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Trigger,
    View,
    Procedure,
    Function,
    Event,
}

impl ObjectKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::View => "view",
            Self::Procedure => "procedure",
            Self::Function => "function",
            Self::Event => "evento",
        }
    }
}

impl fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Definer {
    pub user: String,
    pub host: String,
}

impl fmt::Display for Definer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.user, self.host)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DumpInfo {
    pub database: Option<String>,
    pub dumped_from: Option<MysqlVersion>,
    pub dumped_by: Option<MysqlVersion>,
    pub tables: usize,
    pub views: usize,
    pub triggers: usize,
    pub routines: usize,
    pub events: usize,
    pub definers: BTreeSet<(ObjectKind, Definer)>,
}

impl DumpInfo {
    #[must_use]
    pub fn objects(&self) -> usize {
        self.tables + self.views + self.triggers + self.routines + self.events
    }
}

#[derive(Debug, Error)]
pub enum DumpError {
    #[error("não foi possível ler {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("o arquivo não começa com o cabeçalho do mysqldump")]
    NotMysqlDump,
    #[error("o dump está incompleto: falta a linha final \"-- Dump completed\" do mysqldump")]
    Incomplete,
    #[error(
        "o dump troca ou cria bancos ({statement}); só são aceitos dumps de um banco, sem --databases"
    )]
    SelectsDatabase { statement: String },
    #[error(
        "o {kind} `{name}` tem o corpo terminado em \";\", e o mysqldump gera para ele um comando que o próprio MySQL não consegue restaurar; recrie o {kind} sem o \";\" final (por exemplo, pelo cliente mysql) e gere o backup de novo"
    )]
    UnrestorableObject { kind: ObjectKind, name: String },
}

pub fn inspect_dump(path: &Path) -> Result<DumpInfo, DumpError> {
    let file = std::fs::File::open(path).map_err(|source| DumpError::Io {
        path: path.to_owned(),
        source,
    })?;
    scan(file).map_err(|error| match error {
        ScanError::Io(source) => DumpError::Io {
            path: path.to_owned(),
            source,
        },
        ScanError::Dump(error) => error,
    })
}

enum ScanError {
    Io(io::Error),
    Dump(DumpError),
}

impl From<DumpError> for ScanError {
    fn from(error: DumpError) -> Self {
        Self::Dump(error)
    }
}

struct Tail {
    ring: [u8; KEPT_TAIL_BYTES],
    len: usize,
}

impl Tail {
    fn new() -> Self {
        Self {
            ring: [0; KEPT_TAIL_BYTES],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        self.ring[self.len % KEPT_TAIL_BYTES] = byte;
        self.len += 1;
    }

    fn take(&mut self) -> Vec<u8> {
        let kept = self.len.min(KEPT_TAIL_BYTES);
        let start = self.len - kept;
        let bytes = (start..self.len)
            .map(|index| self.ring[index % KEPT_TAIL_BYTES])
            .collect();
        self.len = 0;
        bytes
    }
}

#[derive(Default)]
struct Scanner {
    info: DumpInfo,
    started: bool,
    last: Vec<u8>,
    open: Option<(ObjectKind, String)>,
}

fn scan(reader: impl Read) -> Result<DumpInfo, ScanError> {
    let mut reader = BufReader::with_capacity(64 * 1024, reader);
    let mut scanner = Scanner::default();
    let mut line = Vec::with_capacity(KEPT_LINE_BYTES);
    let mut tail = Tail::new();

    loop {
        let buffer = reader.fill_buf().map_err(ScanError::Io)?;
        if buffer.is_empty() {
            break;
        }
        let read = buffer.len();
        for &byte in buffer {
            if byte == b'\n' {
                scanner.line(&line, &tail.take())?;
                line.clear();
            } else {
                if line.len() < KEPT_LINE_BYTES {
                    line.push(byte);
                }
                tail.push(byte);
            }
        }
        reader.consume(read);
    }
    if !line.is_empty() {
        scanner.line(&line, &tail.take())?;
    }
    if !scanner.started {
        return Err(DumpError::NotMysqlDump.into());
    }
    if !String::from_utf8_lossy(&scanner.last).starts_with(FOOTER) {
        return Err(DumpError::Incomplete.into());
    }
    Ok(scanner.info)
}

impl Scanner {
    fn line(&mut self, raw: &[u8], tail: &[u8]) -> Result<(), DumpError> {
        let text = String::from_utf8_lossy(raw);
        let line = text.trim_end_matches('\r');
        if !self.started {
            let Some(header) = line.strip_prefix(HEADER) else {
                return Err(DumpError::NotMysqlDump);
            };
            self.info.dumped_by = header
                .split_once("Distrib ")
                .and_then(|(_, rest)| MysqlVersion::parse(rest));
            self.started = true;
        }
        if line.trim().is_empty() {
            return Ok(());
        }
        self.last.clear();
        self.last.extend_from_slice(line.as_bytes());

        if let Some(statement) = DATABASE_STATEMENTS
            .iter()
            .find(|statement| line.starts_with(**statement))
        {
            return Err(DumpError::SelectsDatabase {
                statement: statement.trim().to_owned(),
            });
        }
        self.classify(line);
        self.close_object(tail)
    }

    fn classify(&mut self, line: &str) {
        if let Some(comment) = line.strip_prefix("-- ") {
            if let Some((_, database)) = comment.split_once("Database: ") {
                self.info.database = Some(database.trim().to_owned());
            } else if let Some(version) = comment.strip_prefix("Server version") {
                self.info.dumped_from = MysqlVersion::parse(version);
            }
        } else if line.starts_with("CREATE TABLE `") {
            self.info.tables += 1;
        } else if line.starts_with("/*!50001 VIEW `") {
            self.info.views += 1;
        } else if line.starts_with(VIEW_DEFINER) {
            self.add_definer(ObjectKind::View, line);
        } else if let Some((_, rest)) = line.split_once(TRIGGER_MARKER) {
            self.info.triggers += 1;
            self.open_object(ObjectKind::Trigger, line, rest);
        } else if let Some((_, rest)) = line.split_once(EVENT_MARKER) {
            self.info.events += 1;
            self.open_object(ObjectKind::Event, line, rest);
        } else if line.starts_with("CREATE DEFINER=") {
            let kind = if line.contains(" PROCEDURE `") {
                ObjectKind::Procedure
            } else if line.contains(" FUNCTION `") {
                ObjectKind::Function
            } else {
                return;
            };
            self.info.routines += 1;
            self.add_definer(kind, line);
        }
    }

    fn open_object(&mut self, kind: ObjectKind, line: &str, rest: &str) {
        self.add_definer(kind, line);
        let name = parse_quoted(rest).map_or_else(|| "?".to_owned(), |(name, _)| name);
        self.open = Some((kind, name));
    }

    fn add_definer(&mut self, kind: ObjectKind, line: &str) {
        if let Some(definer) = parse_definer(line) {
            self.info.definers.insert((kind, definer));
        }
    }

    fn close_object(&mut self, tail: &[u8]) -> Result<(), DumpError> {
        let tail = String::from_utf8_lossy(tail);
        let Some(statement) = tail.trim_end().strip_suffix(";;") else {
            return Ok(());
        };
        let Some((kind, name)) = self.open.take() else {
            return Ok(());
        };
        let body = statement.trim_end().strip_suffix("*/").unwrap_or(statement);
        if body.trim_end().ends_with(';') {
            return Err(DumpError::UnrestorableObject { kind, name });
        }
        Ok(())
    }
}

fn parse_definer(line: &str) -> Option<Definer> {
    let (_, rest) = line.split_once("DEFINER=")?;
    let (user, rest) = parse_quoted(rest)?;
    let (host, _) = parse_quoted(rest.strip_prefix('@')?)?;
    Some(Definer { user, host })
}

fn parse_quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('`')?.char_indices().peekable();
    let mut value = String::new();
    while let Some((index, c)) = chars.next() {
        if c != '`' {
            value.push(c);
        } else if chars.peek().is_some_and(|(_, next)| *next == '`') {
            value.push('`');
            chars.next();
        } else {
            return Some((value, &text[index + 2..]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "-- MySQL dump 10.13  Distrib 8.4.11, for Linux (x86_64)
--
-- Host: 127.0.0.1    Database: app
-- ------------------------------------------------------
-- Server version\t8.4.13

/*!40101 SET NAMES utf8mb4 */;
DROP TABLE IF EXISTS `audit_log`;
CREATE TABLE `audit_log` (
  `id` int NOT NULL AUTO_INCREMENT
) ENGINE=InnoDB;
/*!50001 CREATE VIEW `customer_totals` AS SELECT 1 AS `id`*/;
CREATE TABLE `customers` (
  `id` int NOT NULL
) ENGINE=InnoDB;
INSERT INTO `customers` VALUES (1),(2);
DELIMITER ;;
/*!50003 CREATE*/ /*!50017 DEFINER=`root`@`localhost`*/ /*!50003 TRIGGER `orders_audit` AFTER INSERT ON `orders` FOR EACH ROW SET @x = 1 */;;
/*!50003 CREATE*/ /*!50017 DEFINER=`app`@`%`*/ /*!50003 TRIGGER `multi_line` BEFORE INSERT ON `orders` FOR EACH ROW BEGIN
  SET @y = 1;
  SET @z = 2;
END */;;
/*!50106 CREATE*/ /*!50117 DEFINER=`root`@`%`*/ /*!50106 EVENT `e_noop` ON SCHEDULE EVERY 1 DAY DO SELECT 1 */ ;;
CREATE DEFINER=`root`@`%` FUNCTION `f_one`() RETURNS int
CREATE DEFINER=`we``ird`@`%` PROCEDURE `p_count`()
DELIMITER ;
/*!50001 CREATE ALGORITHM=UNDEFINED */
/*!50013 DEFINER=`root`@`localhost` SQL SECURITY DEFINER */
/*!50001 VIEW `customer_totals` AS select 1 AS `id` */;

-- Dump completed on 2026-10-23  8:52:28
";

    fn scan_text(text: &str) -> Result<DumpInfo, DumpError> {
        scan(text.as_bytes()).map_err(|error| match error {
            ScanError::Io(error) => DumpError::Io {
                path: PathBuf::new(),
                source: error,
            },
            ScanError::Dump(error) => error,
        })
    }

    fn definer(user: &str, host: &str) -> Definer {
        Definer {
            user: user.to_owned(),
            host: host.to_owned(),
        }
    }

    #[test]
    fn reads_header_and_counts_objects_of_a_complete_dump() {
        let info = scan_text(DUMP).expect("dump válido");

        assert_eq!(info.database.as_deref(), Some("app"));
        assert_eq!(info.dumped_by, Some(MysqlVersion { major: 8, minor: 4 }));
        assert_eq!(info.dumped_from, Some(MysqlVersion { major: 8, minor: 4 }));
        assert_eq!(
            (
                info.tables,
                info.views,
                info.triggers,
                info.events,
                info.routines
            ),
            (2, 1, 2, 1, 2)
        );
        assert_eq!(info.objects(), 8);
    }

    #[test]
    fn collects_every_definer_by_object_kind() {
        let info = scan_text(DUMP).expect("dump válido");

        let expected: BTreeSet<_> = [
            (ObjectKind::Trigger, definer("root", "localhost")),
            (ObjectKind::Trigger, definer("app", "%")),
            (ObjectKind::View, definer("root", "localhost")),
            (ObjectKind::Function, definer("root", "%")),
            (ObjectKind::Procedure, definer("we`ird", "%")),
            (ObjectKind::Event, definer("root", "%")),
        ]
        .into_iter()
        .collect();
        assert_eq!(info.definers, expected);
    }

    #[test]
    fn trigger_or_event_body_ending_in_semicolon_is_unrestorable() {
        let cases = [
            (
                "SET @x = 1 */;;",
                "SET @x = 1; */;;",
                ObjectKind::Trigger,
                "orders_audit",
            ),
            ("END */;;", "END; */;;", ObjectKind::Trigger, "multi_line"),
            (
                "DO SELECT 1 */ ;;",
                "DO SELECT 1; */ ;;",
                ObjectKind::Event,
                "e_noop",
            ),
        ];
        for (good, bad, kind, name) in cases {
            let text = DUMP.replacen(good, bad, 1);
            match scan_text(&text) {
                Err(DumpError::UnrestorableObject {
                    kind: found,
                    name: found_name,
                }) => {
                    assert_eq!((found, found_name.as_str()), (kind, name), "{bad}");
                }
                other => panic!("{bad}: {other:?}"),
            }
        }
    }

    #[test]
    fn long_trigger_body_is_judged_by_its_end() {
        let body = "x".repeat(KEPT_LINE_BYTES * 4);
        let good = DUMP.replacen("SET @x = 1 */;;", &format!("SET @x = '{body}' */;;"), 1);
        let bad = DUMP.replacen("SET @x = 1 */;;", &format!("SET @x = '{body}'; */;;"), 1);

        assert!(scan_text(&good).is_ok());
        assert!(matches!(
            scan_text(&bad),
            Err(DumpError::UnrestorableObject { .. })
        ));
    }

    #[test]
    fn truncated_dump_without_footer_is_incomplete() {
        let cut = &DUMP[..DUMP.find("INSERT INTO").expect("insert")];
        assert!(matches!(scan_text(cut), Err(DumpError::Incomplete)));
        let crlf = DUMP.replace('\n', "\r\n");
        assert!(scan_text(&crlf).is_ok());
    }

    #[test]
    fn foreign_files_are_not_mysql_dumps() {
        for text in [
            "",
            "PGDMP\u{1}\u{2}",
            "SQLite format 3\0",
            "SELECT 1;\n-- Dump completed\n",
        ] {
            assert!(
                matches!(scan_text(text), Err(DumpError::NotMysqlDump)),
                "{text:?}"
            );
        }
    }

    #[test]
    fn dumps_that_switch_or_create_databases_are_rejected() {
        for statement in [
            "USE `mysql`;",
            "CREATE DATABASE /*!32312 IF NOT EXISTS*/ `x`;",
            "DROP DATABASE `y`;",
        ] {
            let text = DUMP.replace("/*!40101 SET NAMES utf8mb4 */;", statement);
            assert!(
                matches!(scan_text(&text), Err(DumpError::SelectsDatabase { .. })),
                "{statement}"
            );
        }
    }

    #[test]
    fn huge_lines_are_scanned_without_being_kept() {
        let big = format!("INSERT INTO `t` VALUES ({});", "1,".repeat(500_000));
        let text = DUMP.replace("INSERT INTO `customers` VALUES (1),(2);", &big);
        assert_eq!(scan_text(&text).expect("dump válido").tables, 2);
    }
}
