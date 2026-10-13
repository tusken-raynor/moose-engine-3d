//! Writes `moose.d.ts` from the API table: what an editor (VSCode) checks scripts against
//! and completes them from. The engine's own check, when a script loads, is the one that
//! decides; the two read the same table.

use std::fmt::Write;

use crate::api::{ApiDesc, ClassKind, Method};

fn doc(out: &mut String, indent: &str, text: &str) {
    if !text.is_empty() {
        writeln!(out, "{indent}/** {text} */").unwrap();
    }
}

fn signature(desc: &ApiDesc, m: &Method) -> String {
    let mut params: Vec<String> = m.params.iter().map(|(n, t)| format!("{n}: {}", desc.type_name(t))).collect();
    if m.rest {
        params.push("...values: any[]".to_string());
    }
    format!("{}({}): {}", m.name, params.join(", "), desc.type_name(&m.ret))
}

/// The text of `moose.d.ts` for this API.
pub fn declarations(desc: &ApiDesc) -> String {
    let mut out = String::new();
    out.push_str(
        "// Written by the engine from its script API table (moose-app's `scripts` module): don't\n\
         // edit it. To write it again after the table changes:\n\
         //   MOOSE_WRITE_DTS=1 cargo test -p moose-app moose_d_ts\n\
         //\n\
         // Moose scripts are a strict subset of TypeScript. This file is what an editor checks them\n\
         // against; the engine checks them again as it loads them, and its word is the last.\n\
         //\n\
         // A script hears an event by exporting a function of its name:\n",
    );
    for event in &desc.events {
        let params: Vec<String> = event.params.iter().map(|(n, t)| format!("{n}: {}", desc.type_name(t))).collect();
        writeln!(out, "//   export function {}({})", event.name, params.join(", ")).unwrap();
        if !event.doc.is_empty() {
            writeln!(out, "//     {}", event.doc).unwrap();
        }
    }
    out.push_str(
        "\n/** A whole number (an i32 in the engine). To TypeScript it is a number; the engine keeps them apart. */\n\
         type int = number;\n\n\
         /**\n * An input of the script, set where the script is placed (in the level, or on an entity or\n \
         * its template): `const door = input<Entity>();`, or with a fallback, `const speed = input(2.0);`.\n \
         * Only a top-level const can start as one.\n */\n\
         declare function input<T>(fallback?: T): T;\n\n",
    );
    if let Some(class) = desc.self_class {
        let name = &desc.classes[class as usize].name;
        writeln!(out, "/** What the script is attached to. Only in a script placed on an entity or a template. */").unwrap();
        writeln!(out, "declare const self: {name};\n").unwrap();
    }
    for class in desc.classes.iter().filter(|c| !c.ambient) {
        doc(&mut out, "", &class.doc);
        writeln!(out, "interface {} {{", class.name).unwrap();
        for prop in &class.props {
            doc(&mut out, "  ", &prop.doc);
            let readonly = if prop.set.is_none() { "readonly " } else { "" };
            writeln!(out, "  {readonly}{}: {};", prop.name, desc.type_name(&prop.ty)).unwrap();
        }
        for method in &class.methods {
            doc(&mut out, "  ", &method.doc);
            writeln!(out, "  {};", signature(desc, method)).unwrap();
        }
        out.push_str("}\n");
        if class.kind == ClassKind::Singleton {
            for global in desc.globals.iter().filter(|g| desc.classes[g.class as usize].name == class.name) {
                writeln!(out, "declare const {}: {};", global.name, class.name).unwrap();
            }
        }
        out.push('\n');
    }
    for func in &desc.funcs {
        doc(&mut out, "", &func.doc);
        writeln!(out, "declare function {};", signature(desc, func)).unwrap();
    }
    out
}
