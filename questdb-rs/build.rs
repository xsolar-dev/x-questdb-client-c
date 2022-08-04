pub mod json_tests {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::path::PathBuf;
    use serde::{Serialize, Deserialize};
    use serde_json;
    use slugify::slugify;
    use indoc::indoc;

    #[derive(Debug, Serialize, Deserialize)]
    struct Symbol {
        name: String,
        value: String
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct StringColumn {
        name: String,
        value: String,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct LongColumn {
        name: String,
        value: i64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct DoubleColumn {
        name: String,
        value: f64
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct BooleanColumn {
        name: String,
        value: bool
    }


    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "UPPERCASE")]
    enum Column {
        String(StringColumn),
        Long(LongColumn),
        Double(DoubleColumn),
        Boolean(BooleanColumn)
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Expected {
        line: Option<String>,

        #[serde(rename = "anyLines")]
        any_lines: Option<Vec<String>>
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "UPPERCASE")]
    enum Outcome {
        Success(Expected),
        Error
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct TestSpec {
        #[serde(rename = "testName")]
        test_name: String,
        table: String,
        symbols: Vec<Symbol>,
        columns: Vec<Column>,
        result: Outcome
    }

    fn parse() -> Vec<TestSpec> {
        let mut json_path = PathBuf::from(
            std::env::var("CARGO_MANIFEST_DIR").unwrap());
        json_path.push("src");
        json_path.push("tests");
        json_path.push("interop");
        json_path.push("ilp-client-interop-test.json");
        let file = std::fs::File::open(json_path).unwrap();
        serde_json::from_reader(file).unwrap()
    }

    pub fn build() -> Result<(), Box<dyn std::error::Error>> {
        let specs = parse();
        // eprintln!("Parsed JSON: {:#?}", specs);
        let mut file_path = PathBuf::from(std::env::var("OUT_DIR")?);
        file_path.push("json_tests.rs");
        let mut output = BufWriter::new(File::create(file_path)?);
        // let mut output = String::new();
        writeln!(output, "{}", indoc!{r#"
            // This file is auto-generated by build.rs.

            use crate::{Result, ingress::{Buffer}};
            use crate::tests::{TestResult};

            fn matches_any_line(line: &str, expected: &[&str]) -> bool {
                for &exp in expected {
                    if line == exp {
                        return true;
                    }
                }
                eprintln!(
                    "Could not match:\n    {:?}\nTo any of: {:#?}",
                    line, expected);
                false
            }
            "#})?;

        for (index, spec) in specs.iter().enumerate() {
            writeln!(output, "/// {}", spec.test_name)?;
            // for line in serde_json::to_string_pretty(&spec).unwrap().split("\n") {
            //     writeln!(output, "/// {}", line)?;
            // }
            writeln!(output, "#[test]")?;
            writeln!(output, "fn test_{:03}_{}() -> TestResult {{",
                index, slugify!(&spec.test_name, separator="_"))?;
            writeln!(output, "    let mut buffer = Buffer::new();")?;

            let (expected, indent) = match &spec.result {
                Outcome::Success(line) => (Some(line), ""),
                Outcome::Error => (None, "    ")
            };
            if expected.is_none() {
                writeln!(output, "    || -> Result<()> {{")?;
            }
            writeln!(output, "{}    buffer", indent)?;
            writeln!(output, "{}        .table({:?})?", indent, spec.table)?;
            for symbol in spec.symbols.iter() {
                writeln!(output, "{}        .symbol({:?}, {:?})?", indent, symbol.name, symbol.value)?;
            }
            for column in spec.columns.iter() {
                match column {
                    Column::String(column) =>
                        writeln!(output, "{}        .column_str({:?}, {:?})?", indent, column.name, column.value)?,
                    Column::Long(column) =>
                        writeln!(output, "{}        .column_i64({:?}, {:?})?", indent, column.name, column.value)?,
                    Column::Double(column) =>
                        writeln!(output, "{}        .column_f64({:?}, {:?})?", indent, column.name, column.value)?,
                    Column::Boolean(column) =>
                        writeln!(output, "{}        .column_bool({:?}, {:?})?", indent, column.name, column.value)?,
                }
            }
            writeln!(output, "{}        .at_now()?;", indent)?;
            if let Some(ref expected) = expected {
                if let Some(ref line) = expected.line {
                    let exp_ln = format!("{}\n", line);
                    writeln!(output, "    let exp = {:?};", exp_ln)?;
                    writeln!(output, "    assert_eq!(buffer.as_str(), exp);")?;
                }
                else {
                    let any: Vec<String> = expected.any_lines.as_ref().unwrap()
                        .iter()
                        .map(|line| format!("{}\n", line))
                        .collect();
                    writeln!(output, "    let any = [")?;
                    for line in any.iter() {
                        writeln!(output, "            {:?},", line)?;
                    }
                    writeln!(output, "        ];")?;
                    writeln!(output, "    assert!(matches_any_line(buffer.as_str(), &any));")?;
                }
            }
            else {
                writeln!(output, "        Ok(())")?;
                writeln!(output, "    }}().unwrap_err();")?;
            }
            writeln!(output, "    Ok(())")?;
            writeln!(output, "}}")?;
        }
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=src/test/interop/ilp-client-interop-test.json");

    json_tests::build()?;

    Ok(())
}
