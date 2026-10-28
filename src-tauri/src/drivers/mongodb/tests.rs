//! The console's language: splitting, statements as commands (golden documents) and the
//! read/write classifier, with its hard cases and a fuzz.

use ::mongodb::bson::{Bson, Document, Regex, doc, oid::ObjectId};

use super::classify::{classify_command, classify_script, classify_text};
use super::shell::{Statement, parse, split};
use crate::sqlgen::classify::StatementClass::{self, Read, Write};
use crate::sqlgen::native::mongo::{ConsoleCommand, Output};

fn run(text: &str) -> ConsoleCommand {
    match parse(text, "shop") {
        Ok(Statement::Run(c)) => c,
        other => panic!("{text}: {other:?}"),
    }
}

fn cmd(text: &str) -> Document {
    run(text).command
}

fn err(text: &str) -> String {
    match parse(text, "shop") {
        Err(e) => e.message,
        Ok(st) => panic!("{text}: parsed as {st:?}"),
    }
}

fn sqls(text: &str) -> Vec<String> {
    split(text).into_iter().map(|s| s.sql).collect()
}

// ---------------------------------------------------------------------------------------------
// Splitting
// ---------------------------------------------------------------------------------------------

#[test]
fn statements_end_at_semicolons_and_complete_lines() {
    assert_eq!(
        sqls("db.a.find(); db.b.find()\ndb.c.find({\n  x: 1\n})\nuse other\nshow dbs"),
        [
            "db.a.find()",
            "db.b.find()",
            "db.c.find({\n  x: 1\n})",
            "use other",
            "show dbs"
        ]
    );
    // A line that starts with `.` continues the chain, past comments and blank lines.
    assert_eq!(
        sqls("db.a.find()\n  .sort({x: 1}) // newest\n\n  /* then */ .limit(5)\ndb.b.count()"),
        [
            "db.a.find()\n  .sort({x: 1}) // newest\n\n  /* then */ .limit(5)",
            "db.b.count()"
        ]
    );
    // A trailing dot or comma waits for the next line.
    assert_eq!(sqls("db.a.\nfind()"), ["db.a.\nfind()"]);
}

#[test]
fn separators_inside_strings_regexes_and_comments_do_not_split() {
    assert_eq!(
        sqls(
            "db.a.find({s: 'x;y\\'z', t: \"a\\nb;\", r: /;\\/[/;]/i}) // ; no\n/* ; */ db.b.find()"
        ),
        [
            "db.a.find({s: 'x;y\\'z', t: \"a\\nb;\", r: /;\\/[/;]/i})",
            "db.b.find()"
        ]
    );
    assert_eq!(
        sqls("db.a.insertOne({t: `multi\nline;`})"),
        ["db.a.insertOne({t: `multi\nline;`})"]
    );
}

#[test]
fn offsets_are_characters_and_lines_count_from_one() {
    let text = "// é\nuse é; db.ä.find()\n\n  db.b.find()";
    let parts = split(text);
    assert_eq!(parts.len(), 3);
    let chars: Vec<char> = text.chars().collect();
    for p in &parts {
        assert_eq!(chars[p.start..p.end].iter().collect::<String>(), p.sql);
    }
    assert_eq!(parts.iter().map(|p| p.line).collect::<Vec<_>>(), [2, 2, 4]);
}

#[test]
fn empty_and_comment_only_texts_have_no_statements() {
    assert!(split("").is_empty());
    assert!(split("  ;;\n // x\n /* y */ ;").is_empty());
    // An unterminated string or comment still yields the text, for the parser to report.
    assert_eq!(sqls("db.a.find({s: 'x"), ["db.a.find({s: 'x"]);
    assert_eq!(sqls("db.a.find() /* open"), ["db.a.find()"]);
}

// ---------------------------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------------------------

#[test]
fn find_with_its_cursor_chain() {
    let c = run(
        "db.orders.find({status: 'A', qty: {$gt: 5}}, {item: 1})\n  .sort({qty: -1}).skip(10).limit(20)\n  .hint('qty_1').maxTimeMS(500).comment('x').pretty()",
    );
    assert_eq!(c.db, "shop");
    assert_eq!(c.output, Output::Cursor);
    assert_eq!(
        c.command,
        doc! {
            "find": "orders",
            "filter": { "status": "A", "qty": { "$gt": 5 } },
            "projection": { "item": 1 },
            "sort": { "qty": -1 },
            "skip": 10_i64,
            "limit": 20_i64,
            "hint": "qty_1",
            "maxTimeMS": 500_i64,
            "comment": "x",
        }
    );
    assert_eq!(cmd("db.c.find()"), doc! { "find": "c", "filter": {} });
    assert_eq!(
        cmd("db.c.find(null, {})"),
        doc! { "find": "c", "filter": {} }
    );
    assert_eq!(
        cmd("db.c.find().limit(-3)"),
        doc! { "find": "c", "filter": {}, "limit": 3_i64, "singleBatch": true }
    );
    assert_eq!(
        cmd("db.c.find({}, {}, {sort: {a: 1}, limit: 2})"),
        doc! { "find": "c", "filter": {}, "sort": { "a": 1 }, "limit": 2 }
    );
    assert!(err("db.c.find({}, {}, {sortt: 1})").contains("unknown option `sortt`"));
    assert!(err("db.c.find().forEach(printjson)").contains("callbacks"));
    assert!(err("db.c.find().skip(-1)").contains("0 or more"));
}

#[test]
fn cursor_counts_and_explains() {
    let c = run("db.c.find({a: 1}).skip(5).limit(2).count()");
    assert_eq!(c.output, Output::Count);
    assert_eq!(c.command, doc! { "count": "c", "query": { "a": 1 } });
    assert_eq!(
        cmd("db.c.find({a: 1}).skip(5).limit(2).size()"),
        doc! { "count": "c", "query": { "a": 1 }, "skip": 5_i64, "limit": 2_i64 }
    );
    let e = run("db.c.find({a: 1}).explain('executionStats')");
    assert_eq!(e.output, Output::Reply);
    assert_eq!(
        e.command,
        doc! { "explain": { "find": "c", "filter": { "a": 1 } }, "verbosity": "executionStats" }
    );
    assert_eq!(
        cmd("db.c.explain().find().limit(1)"),
        doc! { "explain": { "find": "c", "filter": {}, "limit": 1_i64 }, "verbosity": "queryPlanner" }
    );
    assert_eq!(
        cmd("db.c.explain(true).aggregate([{$match: {}}])"),
        doc! { "explain": { "aggregate": "c", "pipeline": [{ "$match": {} }], "cursor": {} }, "verbosity": "allPlansExecution" }
    );
    assert_eq!(
        cmd("db.c.aggregate([]).explain()"),
        doc! { "explain": { "aggregate": "c", "pipeline": [], "cursor": {} }, "verbosity": "queryPlanner" }
    );
    assert!(
        err("db.c.explain('fast').find()").contains("verbosity")
            || err("db.c.explain('fast').find()").contains("queryPlanner")
    );
}

#[test]
fn reads_of_one_value() {
    let c = run("db.users.findOne({_id: ObjectId('65f1a2b3c4d5e6f708192a3b')}, {name: 1})");
    assert_eq!(c.output, Output::First);
    assert_eq!(
        c.command,
        doc! {
            "find": "users",
            "filter": { "_id": ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap() },
            "projection": { "name": 1 },
            "limit": 1_i64,
            "singleBatch": true,
        }
    );
    let c = run("db.c.countDocuments({a: 1}, {skip: 1, limit: 5, maxTimeMS: 100})");
    assert_eq!(c.output, Output::CountDocuments);
    assert_eq!(
        c.command,
        doc! {
            "aggregate": "c",
            "pipeline": [
                { "$match": { "a": 1 } }, { "$skip": 1_i64 }, { "$limit": 5_i64 },
                { "$group": { "_id": 1, "n": { "$sum": 1 } } },
            ],
            "cursor": {},
            "maxTimeMS": 100,
        }
    );
    assert_eq!(cmd("db.c.estimatedDocumentCount()"), doc! { "count": "c" });
    assert_eq!(
        cmd("db.c.count({a: 1})"),
        doc! { "count": "c", "query": { "a": 1 } }
    );
    let d = run("db.c.distinct('tags', {a: {$exists: true}})");
    assert_eq!(d.output, Output::Distinct);
    assert_eq!(
        d.command,
        doc! { "distinct": "c", "key": "tags", "query": { "a": { "$exists": true } } }
    );
}

#[test]
fn aggregations() {
    let c = run(
        "db.sales.aggregate([{$match: {y: 2024}}, {$group: {_id: '$k', n: {$sum: 1}}}], {allowDiskUse: true, batchSize: 10})",
    );
    assert_eq!(c.output, Output::Cursor);
    assert_eq!(
        c.command,
        doc! {
            "aggregate": "sales",
            "pipeline": [{ "$match": { "y": 2024 } }, { "$group": { "_id": "$k", "n": { "$sum": 1 } } }],
            "cursor": { "batchSize": 10 },
            "allowDiskUse": true,
        }
    );
    // The old spelling with stages as arguments.
    assert_eq!(
        cmd("db.c.aggregate({$match: {}}, {$limit: 1})"),
        doc! { "aggregate": "c", "pipeline": [{ "$match": {} }, { "$limit": 1 }], "cursor": {} }
    );
    let e = run("db.c.aggregate([], {explain: true})");
    assert_eq!(e.output, Output::Reply);
    assert_eq!(
        e.command,
        doc! { "aggregate": "c", "pipeline": [], "explain": true }
    );
    assert_eq!(
        cmd("db.aggregate([{$currentOp: {}}])"),
        doc! { "aggregate": 1, "pipeline": [{ "$currentOp": {} }], "cursor": {} }
    );
}

#[test]
fn inserts_give_ids_first() {
    let c = run("db.c.insertOne({a: 1, _id: 'mine'})");
    assert_eq!(
        c.output,
        Output::Inserted(vec![Bson::String("mine".into())])
    );
    assert_eq!(
        c.command,
        doc! { "insert": "c", "documents": [{ "_id": "mine", "a": 1 }], "ordered": true }
    );
    let c = run("db.c.insertMany([{a: 1}, {b: 2}], {ordered: false})");
    let Output::Inserted(ids) = &c.output else {
        panic!("{:?}", c.output)
    };
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| matches!(id, Bson::ObjectId(_))));
    let docs = c.command.get_array("documents").unwrap();
    assert_eq!(docs[0].as_document().unwrap().get("_id"), Some(&ids[0]));
    assert_eq!(c.command.get_bool("ordered"), Ok(false));
    assert_eq!(
        cmd("db.c.insert({x: 1})")
            .get_array("documents")
            .unwrap()
            .len(),
        1
    );
    assert!(err("db.c.insertMany([])").contains("at least one"));
    assert!(err("db.c.insertMany([1])").contains("documents"));
    assert!(err("db.c.insertOne({}, {ordered: true})").contains("unknown option"));
}

#[test]
fn updates_and_replacements() {
    let c = run("db.c.updateOne({_id: 1}, {$set: {a: 2}}, {upsert: true, writeConcern: {w: 1}})");
    assert_eq!(c.output, Output::Updated);
    assert_eq!(
        c.command,
        doc! {
            "update": "c",
            "updates": [{ "q": { "_id": 1 }, "u": { "$set": { "a": 2 } }, "multi": false, "upsert": true }],
            "ordered": true,
            "writeConcern": { "w": 1 },
        }
    );
    assert_eq!(
        cmd("db.c.updateMany({}, [{$set: {n: {$add: ['$n', 1]}}}])"),
        doc! { "update": "c", "updates": [{ "q": {}, "u": [{ "$set": { "n": { "$add": ["$n", 1] } } }], "multi": true }], "ordered": true }
    );
    assert_eq!(
        cmd("db.c.replaceOne({_id: 1}, {a: 1})"),
        doc! { "update": "c", "updates": [{ "q": { "_id": 1 }, "u": { "a": 1 }, "multi": false }], "ordered": true }
    );
    assert_eq!(
        cmd("db.c.update({}, {$inc: {n: 1}}, {multi: true})"),
        doc! { "update": "c", "updates": [{ "q": {}, "u": { "$inc": { "n": 1 } }, "multi": true }], "ordered": true }
    );
    // A replacement by mistake (or operators in a replacement) is refused, as mongosh does.
    assert!(err("db.c.updateOne({}, {a: 1})").contains("$set"));
    assert!(err("db.c.replaceOne({}, {$set: {a: 1}})").contains("replacement"));
    assert!(err("db.c.updateOne({}, {})").contains("$set"));
    assert!(err("db.c.updateOne({}, {$set: {a: 1}}, {multi: true})").contains("multi"));
}

#[test]
fn deletes() {
    let c = run("db.c.deleteOne({a: 1}, {hint: 'a_1'})");
    assert_eq!(c.output, Output::Deleted);
    assert_eq!(
        c.command,
        doc! { "delete": "c", "deletes": [{ "q": { "a": 1 }, "limit": 1, "hint": "a_1" }], "ordered": true }
    );
    assert_eq!(
        cmd("db.c.deleteMany({})"),
        doc! { "delete": "c", "deletes": [{ "q": {}, "limit": 0 }], "ordered": true }
    );
    assert_eq!(
        cmd("db.c.remove({a: 1}, true)"),
        doc! { "delete": "c", "deletes": [{ "q": { "a": 1 }, "limit": 1 }], "ordered": true }
    );
    assert_eq!(
        cmd("db.c.remove({a: 1}, {justOne: false})"),
        doc! { "delete": "c", "deletes": [{ "q": { "a": 1 }, "limit": 0 }], "ordered": true }
    );
    assert!(err("db.c.deleteOne()").contains("takes 1 argument"));
}

#[test]
fn find_and_modify_forms() {
    let c = run(
        "db.c.findOneAndUpdate({a: 1}, {$inc: {n: 1}}, {returnDocument: 'after', projection: {n: 1}, sort: {a: 1}})",
    );
    assert_eq!(c.output, Output::Modified);
    assert_eq!(
        c.command,
        doc! {
            "findAndModify": "c",
            "query": { "a": 1 },
            "update": { "$inc": { "n": 1 } },
            "fields": { "n": 1 },
            "new": true,
            "sort": { "a": 1 },
        }
    );
    assert_eq!(
        cmd("db.c.findOneAndDelete({a: 1})"),
        doc! { "findAndModify": "c", "query": { "a": 1 }, "remove": true }
    );
    assert_eq!(
        cmd("db.c.findOneAndReplace({a: 1}, {b: 2}, {returnNewDocument: true})"),
        doc! { "findAndModify": "c", "query": { "a": 1 }, "update": { "b": 2 }, "new": true }
    );
    assert_eq!(
        cmd("db.c.findAndModify({query: {a: 1}, remove: true})"),
        doc! { "findAndModify": "c", "query": { "a": 1 }, "remove": true }
    );
    assert!(err("db.c.findAndModify({query: {}})").contains("update"));
}

#[test]
fn bulk_writes_group_runs_of_one_kind() {
    let st = parse(
        "db.c.bulkWrite([{insertOne: {document: {a: 1}}}, {insertOne: {document: {_id: 2}}}, \
         {updateOne: {filter: {a: 1}, update: {$set: {b: 1}}}}, {deleteMany: {filter: {}}}], {ordered: false})",
        "shop",
    )
    .unwrap();
    let Statement::Batch { commands, ordered } = st else {
        panic!()
    };
    assert!(!ordered);
    let names: Vec<&str> = commands
        .iter()
        .map(|c| c.command.keys().next().unwrap().as_str())
        .collect();
    assert_eq!(names, ["insert", "update", "delete"]);
    assert_eq!(commands[0].command.get_array("documents").unwrap().len(), 2);
    assert_eq!(
        commands[2].command,
        doc! { "delete": "c", "deletes": [{ "q": {}, "limit": 0 }], "ordered": false }
    );
    assert!(matches!(&commands[0].output, Output::Inserted(ids) if ids.len() == 2));
    assert!(err("db.c.bulkWrite([{insertOne: {doc: {}}}])").contains("document"));
    assert!(err("db.c.bulkWrite([{upsertOne: {}}])").contains("upsertOne"));
}

#[test]
fn indexes_and_collections() {
    let c = run("db.c.createIndex({a: 1, b: -1}, {unique: true})");
    assert_eq!(c.output, Output::Value(Bson::String("a_1_b_-1".into())));
    assert_eq!(
        c.command,
        doc! { "createIndexes": "c", "indexes": [{ "key": { "a": 1, "b": -1 }, "unique": true, "name": "a_1_b_-1" }] }
    );
    let c = run("db.c.createIndexes([{a: 1}, {t: 'text'}])");
    assert_eq!(
        c.output,
        Output::Value(Bson::Array(vec!["a_1".into(), "t_text".into()]))
    );
    assert_eq!(
        cmd("db.c.dropIndex('a_1')"),
        doc! { "dropIndexes": "c", "index": "a_1" }
    );
    assert_eq!(
        cmd("db.c.dropIndexes()"),
        doc! { "dropIndexes": "c", "index": "*" }
    );
    assert_eq!(
        cmd("db.c.getIndexes()"),
        doc! { "listIndexes": "c", "cursor": {} }
    );
    assert_eq!(cmd("db.c.drop()"), doc! { "drop": "c" });
    let r = run("db.c.renameCollection('d', true)");
    assert_eq!(r.db, "admin");
    assert_eq!(
        r.command,
        doc! { "renameCollection": "shop.c", "to": "shop.d", "dropTarget": true }
    );
    assert_eq!(run("db.c.stats()").output, Output::First);
    assert_eq!(
        cmd("db.c.hideIndex('a_1')"),
        doc! { "collMod": "c", "index": { "name": "a_1", "hidden": true } }
    );
    assert_eq!(
        cmd("db.c.validate(true)"),
        doc! { "validate": "c", "full": true }
    );
}

#[test]
fn collection_names_in_every_spelling() {
    assert_eq!(
        cmd("db.system.profile.find()"),
        doc! { "find": "system.profile", "filter": {} }
    );
    assert_eq!(
        cmd("db['my-coll'].find()"),
        doc! { "find": "my-coll", "filter": {} }
    );
    assert_eq!(
        cmd("db.getCollection('a.b').count()"),
        doc! { "count": "a.b" }
    );
    // A collection named like a database method.
    assert_eq!(
        cmd("db.stats.find()"),
        doc! { "find": "stats", "filter": {} }
    );
    let c = run("db.getSiblingDB('logs').events.find()");
    assert_eq!(c.db, "logs");
    let c = run("db.getSiblingDB('logs').getSiblingDB('x')['y'].find()");
    assert_eq!((c.db.as_str(), c.command.get_str("find")), ("x", Ok("y")));
    assert!(err("db.c").contains("method call"));
    assert!(err("db.getSiblingDB('a b').c.find()").contains("invalid database name"));
    assert!(err("db.c.frobnicate()").contains("not a collection method"));
}

#[test]
fn database_methods() {
    let c = run("db.runCommand({ping: 1})");
    assert_eq!((c.db.as_str(), c.output.clone()), ("shop", Output::Reply));
    assert_eq!(c.command, doc! { "ping": 1 });
    assert_eq!(cmd("db.runCommand('buildInfo')"), doc! { "buildInfo": 1 });
    assert_eq!(run("db.adminCommand({listDatabases: 1})").db, "admin");
    let c = run("db.getCollectionNames()");
    assert_eq!(c.output, Output::Names);
    assert_eq!(c.command.get_bool("nameOnly"), Ok(true));
    assert_eq!(
        cmd("db.getCollectionInfos({type: 'view'})"),
        doc! { "listCollections": 1, "nameOnly": false, "authorizedCollections": true, "filter": { "type": "view" } }
    );
    assert_eq!(
        cmd("db.stats(1024)"),
        doc! { "dbStats": 1, "scale": 1024_i64 }
    );
    assert_eq!(run("db.version()").output, Output::Field("version"));
    assert_eq!(
        cmd("db.createCollection('logs', {capped: true, size: 4096})"),
        doc! { "create": "logs", "capped": true, "size": 4096 }
    );
    assert_eq!(
        cmd("db.createView('v', 'c', [{$match: {a: 1}}])"),
        doc! { "create": "v", "viewOn": "c", "pipeline": [{ "$match": { "a": 1 } }] }
    );
    assert_eq!(cmd("db.dropDatabase()"), doc! { "dropDatabase": 1 });
    let k = run("db.killOp(12345)");
    assert_eq!(
        (k.db.as_str(), k.command),
        ("admin", doc! { "killOp": 1, "op": 12345 })
    );
    assert_eq!(
        run("db.currentOp(true)").command,
        doc! { "currentOp": 1, "$all": true }
    );
    assert_eq!(run("rs.status()").command, doc! { "replSetGetStatus": 1 });
    assert!(err("db.eval('x')").contains("not a database method"));
    assert!(err("db.runCommand()").contains("takes 1 argument"));
}

#[test]
fn use_and_show() {
    assert_eq!(
        parse("use reports", "shop").unwrap(),
        Statement::Use("reports".into())
    );
    assert_eq!(
        parse("use reports;", "shop").unwrap(),
        Statement::Use("reports".into())
    );
    assert!(err("use").contains("invalid database name"));
    assert!(err("use a.b").contains("invalid database name"));
    assert!(err("use a b").contains("end of the statement"));
    let dbs = run("show dbs");
    assert_eq!(
        (dbs.db.as_str(), dbs.output),
        ("admin", Output::Field("databases"))
    );
    assert_eq!(run("show collections").output, Output::Names);
    assert_eq!(run("show users").output, Output::Field("users"));
    assert_eq!(run("show roles").output, Output::Field("roles"));
    assert!(err("show profile").contains("show takes"));
}

#[test]
fn javascript_is_refused_with_a_clear_message() {
    assert!(
        err("const x = 1").contains("no JavaScript")
            || err("const x = 1").contains("without JavaScript")
    );
    assert!(err("for (;;) {}").contains("db.…"));
    assert!(err("db.c.find({a: x})").contains("no JavaScript variables"));
    assert!(err("db.c.find(function() { return 1 })").contains("no JavaScript"));
    assert!(err("db.c.find().map(d => d)").contains("callbacks"));
    assert!(err("db.c.find() db.d.find()").contains("end of the statement"));
}

#[test]
fn shell_values_reach_the_command() {
    let c = cmd(
        "db.c.find({d: {$gte: ISODate('2024-01-01')}, n: NumberLong(5), r: /^a/i, x: NumberDecimal('1.5')})",
    );
    let f = c.get_document("filter").unwrap();
    assert!(matches!(
        f.get_document("d").unwrap().get("$gte"),
        Some(Bson::DateTime(_))
    ));
    assert_eq!(f.get("n"), Some(&Bson::Int64(5)));
    assert_eq!(
        f.get("r"),
        Some(&Bson::RegularExpression(Regex {
            pattern: "^a".into(),
            options: "i".into()
        }))
    );
    assert!(matches!(f.get("x"), Some(Bson::Decimal128(_))));
}

#[test]
fn console_text_reads_back_as_the_same_command() {
    for text in [
        "db.c.updateOne({_id: ObjectId('65f1a2b3c4d5e6f708192a3b')}, {$set: {n: NumberLong(7), d: new Date(0)}})",
        "db.getSiblingDB('x').c.deleteMany({a: {$in: [1, 'b', null]}})",
        "db.c.aggregate([{$match: {r: /a\"b/s}}])",
    ] {
        let c = run(text);
        let back = run(&c.text());
        assert_eq!(back.db, c.db, "{text}");
        assert_eq!(back.command, c.command, "{text}");
    }
}

// ---------------------------------------------------------------------------------------------
// Classifier
// ---------------------------------------------------------------------------------------------

#[test]
fn reads_are_an_allow_list() {
    for text in [
        "db.c.find({a: 1}).sort({b: 1}).limit(5)",
        "db.c.findOne()",
        "db.c.countDocuments({})",
        "db.c.estimatedDocumentCount()",
        "db.c.count()",
        "db.c.distinct('a')",
        "db.c.aggregate([{$match: {}}, {$group: {_id: '$a'}}])",
        "db.c.aggregate([{$lookup: {from: 'd', pipeline: [{$match: {}}], as: 'x'}}])",
        "db.c.getIndexes()",
        "db.c.stats()",
        "db.c.explain().find({})",
        "db.c.find().explain('executionStats')",
        "db.c.explain().aggregate([{$match: {}}])",
        "db.c.find().count()",
        "db.getCollectionNames()",
        "db.getCollectionInfos()",
        "db.stats()",
        "db.serverStatus()",
        "db.version()",
        "db.hello()",
        "db.getUsers()",
        "db.runCommand({listCollections: 1})",
        "db.runCommand({find: 'c', filter: {}})",
        "db.runCommand({explain: {count: 'c'}})",
        "db.adminCommand({listDatabases: 1})",
        "db.runCommand('ping')",
        "db.currentOp()",
        "rs.status()",
        "use reports",
        "show dbs",
        "show collections",
        "db.c.find({note: 'drop insert update $out $merge'})",
        "db.c.find({$where: 'this.a > 1'})",
    ] {
        assert_eq!(classify_text(text), Read, "{text}");
    }
}

#[test]
fn writes_and_unknowns_are_writes() {
    for text in [
        "db.c.insertOne({})",
        "db.c.insertMany([{}])",
        "db.c.updateOne({}, {$set: {a: 1}})",
        "db.c.replaceOne({}, {})",
        "db.c.deleteMany({})",
        "db.c.findOneAndUpdate({}, {$set: {a: 1}})",
        "db.c.findOneAndDelete({})",
        "db.c.bulkWrite([{insertOne: {document: {}}}])",
        "db.c.createIndex({a: 1})",
        "db.c.dropIndex('a_1')",
        "db.c.drop()",
        "db.c.renameCollection('d')",
        "db.c.validate()",
        "db.c.hideIndex('a_1')",
        "db.createCollection('x')",
        "db.dropDatabase()",
        "db.killOp(1)",
        "db.c.aggregate([{$out: 'd'}])",
        "db.c.aggregate([{$merge: {into: 'd'}}])",
        "db.c.aggregate([{$match: {}}, {$out: {db: 'x', coll: 'y'}}])",
        "db.c.aggregate([{$facet: {a: [{$out: 'd'}]}}])",
        "db.c.aggregate([{$lookup: {from: 'd', pipeline: [{$merge: 'e'}], as: 'x'}}])",
        "db.c.aggregate([{$unionWith: {coll: 'd', pipeline: [{$out: 'e'}]}}])",
        "db.c.aggregate([{$out: 'd'}], {explain: true})",
        "db.aggregate([{$documents: [{}]}, {$out: 'd'}])",
        "db.c.explain().aggregate([{$out: 'd'}])",
        "db.c.explain().remove({})",
        "db.c.explain().updateOne({}, {$set: {a: 1}})",
        "db.c.explain().findOneAndDelete({})",
        "db.runCommand({insert: 'c', documents: [{}]})",
        "db.runCommand({explain: {delete: 'c', deletes: []}})",
        "db.runCommand({explain: {explain: {find: 'c'}}})",
        "db.runCommand({explain: 'find'})",
        "db.runCommand({aggregate: 'c', pipeline: [{$out: 'x'}], cursor: {}})",
        "db.runCommand({mapReduce: 'c', map: 'f', reduce: 'g', out: {inline: 1}})",
        "db.runCommand({shutdown: 1})",
        "db.runCommand({getMore: 1, collection: 'c'})",
        "db.runCommand({killCursors: 'c', cursors: []})",
        "db.runCommand({Find: 'c'})",
        "db.runCommand({eval: 'x'})",
        "db.adminCommand({setParameter: 1, x: 1})",
        "db.adminCommand({fsync: 1, lock: true})",
        "db.runCommand({profile: 2})",
        "db.runCommand({collMod: 'c'})",
        "db.runCommand({createUser: 'u', pwd: 'p', roles: []})",
        "db.c.find(",
        "db.c.find({a: x})",
        "const x = db.c.find()",
        "x",
        "",
        "  ",
        "db.c.watch()",
    ] {
        assert_eq!(classify_text(text), Write, "{text}");
    }
}

#[test]
fn the_command_name_is_the_first_key() {
    // The server dispatches on the first key; a read key later changes nothing.
    assert_eq!(
        classify_command(&doc! { "insert": "c", "find": "c" }),
        Write
    );
    assert_eq!(classify_command(&doc! { "find": "c", "insert": "c" }), Read);
    assert_eq!(classify_command(&doc! {}), Write);
    // `$out` anywhere in an aggregate, also inside `let` or a `$literal`, is a write.
    assert_eq!(
        classify_command(
            &doc! { "aggregate": "c", "pipeline": [], "cursor": {}, "let": { "x": { "$out": 1 } } }
        ),
        Write
    );
    let mut deep = doc! { "$match": {} };
    for _ in 0..300 {
        deep = doc! { "a": deep };
    }
    assert_eq!(
        classify_command(&doc! { "aggregate": "c", "pipeline": [deep], "cursor": {} }),
        Write
    );
}

#[test]
fn scripts_are_reads_only_when_every_statement_reads() {
    assert_eq!(
        classify_script("use x\ndb.c.find()\nshow collections"),
        Read
    );
    assert_eq!(classify_script("db.c.find()\ndb.c.deleteMany({})"), Write);
    assert_eq!(classify_script("db.c.find(); db.c.drop()"), Write);
    assert_eq!(classify_script("db.c.find()\n  .limit(1)"), Read);
    assert_eq!(classify_script("// only a comment"), Write);
    assert_eq!(classify_script(""), Write);
    // A write hidden after a line break that does not continue the chain.
    assert_eq!(classify_script("db.c.find()\ndb.c.insertOne({})"), Write);
    // A write after a string that closes on the same line.
    assert_eq!(
        classify_script("db.c.find({a: 'x\\'y'}); db.c.drop()"),
        Write
    );
}

/// Random scripts over the console's vocabulary: no panic, a write anywhere makes the script a
/// write, and splitting keeps every statement's text.
#[test]
fn fuzz_never_panics_and_writes_win() {
    let atoms = [
        "db",
        ".",
        "c",
        "find",
        "(",
        ")",
        "{",
        "}",
        "[",
        "]",
        "a",
        ":",
        "1",
        ",",
        "'",
        "\"",
        "`",
        "/",
        "*",
        ";",
        "\n",
        " ",
        "$out",
        "$merge",
        "insertOne",
        "aggregate",
        "drop",
        "use",
        "show",
        "dbs",
        "ObjectId('65f1a2b3c4d5e6f708192a3b')",
        "//",
        "/*",
        "*/",
        "\\",
        "é",
        "runCommand",
        "explain",
        "getSiblingDB",
        "x",
        "-",
        "2",
        "e",
        "NaN",
        "new",
        "Date",
    ];
    let mut seed: u64 = 0x6d6f6e676f;
    let mut next = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    };
    for _ in 0..3000 {
        let len = next() % 24;
        let text: String = (0..len).map(|_| atoms[next() % atoms.len()]).collect();
        let parts = split(&text);
        let chars: Vec<char> = text.chars().collect();
        for p in &parts {
            assert!(p.start <= p.end && p.end <= chars.len(), "{text:?}");
            assert_eq!(chars[p.start..p.end].iter().collect::<String>(), p.sql);
        }
        for p in &parts {
            let _ = parse(&p.sql, "fuzz");
        }
        // A write after any text (a block comment left open is closed first) makes the script
        // a write: it is a statement of its own, or it spoils the one it joins.
        let script = format!("{text}\n*/\n;db.c.insertOne({{}})");
        assert_eq!(classify_script(&script), Write, "{script:?}");
    }
}

fn class_of(text: &str) -> StatementClass {
    classify_script(text)
}

#[test]
fn red_team_cases() {
    // Each of these once looked like a read to someone; they all write.
    for text in [
        // A newline inside the call does not end the statement, the next line still writes.
        "db.c.find({\n})\ndb.c.drop()",
        // A comment that seems to swallow the rest of the line.
        "db.c.find() // ; \ndb.c.drop()",
        // A block comment that closes before the write.
        "db.c.find() /* a */ ; db.c.drop()",
        // A regex that holds a quote.
        "db.c.find({a: /'/}); db.c.drop()",
        // A dotted collection name that ends in a write method.
        "db.system.profile.drop()",
        // `getSiblingDB` chains.
        "db.getSiblingDB('admin').runCommand({shutdown: 1})",
        // The explain of a write.
        "db.runCommand({explain: {update: 'c', updates: [{q: {}, u: {$set: {a: 1}}}]}})",
        // A pipeline update is still an update.
        "db.c.updateMany({}, [{$set: {a: 1}}])",
        // `$merge` as the value of an aggregate option key.
        "db.c.aggregate([{$match: {}}, {'$merge': 'x'}])",
    ] {
        assert_eq!(class_of(text), Write, "{text}");
    }
}

// ---------------------------------------------------------------------------------------------
// End to end: the builders against the fake server, in memory and through the real driver
// ---------------------------------------------------------------------------------------------

mod end_to_end {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use ::mongodb::bson::{Bson, Document, Regex, doc, oid::ObjectId};
    use futures::TryStreamExt as _;

    use crate::drivers::mongodb::testing::FakeMongo;
    use crate::drivers::mongodb::{catalog, errors, schema, shell, values};
    use crate::error::AppError;
    use crate::sqlgen::native::mongo::{self, LoadedTypes};
    use crate::types::browse::{Conjunction, FilterGroup, FilterNode, FilterOp, RowKey};
    use crate::types::mutate::StagedChange;
    use crate::types::schema::TableRef;
    use crate::types::value::Value;

    fn people() -> FakeMongo {
        let f = FakeMongo::new();
        f.seed(
            "hr",
            "people",
            vec![
                doc! { "_id": 1, "name": "Ann", "age": 31, "big": 9_007_199_254_740_993_i64 },
                doc! { "_id": 2, "name": "bob", "age": 25_i64 },
                doc! { "_id": "x3", "name": "Cy", "age": "unknown", "tags": ["a"] },
            ],
        );
        f
    }

    fn ids(reply: &Document) -> Vec<Bson> {
        catalog::cursor_batch(reply)
            .0
            .into_iter()
            .filter_map(|d| d.get("_id").cloned())
            .collect()
    }

    fn find_all(f: &FakeMongo, filter: Document) -> Vec<Bson> {
        let spec = mongo::FindSpec {
            filter,
            ..Default::default()
        };
        ids(&f.run("hr", &mongo::find("people", &spec)))
    }

    #[test]
    fn filter_bar_documents_select_what_they_say() {
        let f = people();
        let sample = f.docs("hr", "people");
        let cols = schema::columns(&schema::infer(&sample), sample.len());
        let run = |op: FilterOp, column: &str, values: Vec<Value>| {
            let group = FilterGroup {
                conjunction: Conjunction::And,
                items: vec![FilterNode::Condition {
                    column: column.into(),
                    op,
                    values,
                }],
            };
            find_all(&f, mongo::filter_document(Some(&group), &cols).unwrap())
        };
        // `age` holds ints, a long and a string: 25 typed as text finds the long.
        assert_eq!(
            run(FilterOp::Eq, "age", vec![Value::Text("25".into())]),
            [Bson::Int32(2)]
        );
        assert_eq!(
            run(FilterOp::Gt, "age", vec![Value::Int(26)]),
            [Bson::Int32(1)]
        );
        assert_eq!(
            run(FilterOp::Eq, "age", vec![Value::Text("unknown".into())]),
            [Bson::String("x3".into())]
        );
        assert_eq!(
            run(FilterOp::Contains, "name", vec![Value::Text("o".into())]),
            [Bson::Int32(2)]
        );
        assert_eq!(
            run(
                FilterOp::ContainsCase,
                "name",
                vec![Value::Text("AN".into())]
            ),
            [Bson::Int32(1)]
        );
        assert_eq!(
            run(FilterOp::Like, "name", vec![Value::Text("_y".into())]),
            [Bson::String("x3".into())]
        );
        assert_eq!(run(FilterOp::IsNull, "tags", vec![]).len(), 2);
        assert_eq!(
            run(FilterOp::Eq, "_id", vec![Value::Text("x3".into())]),
            [Bson::String("x3".into())]
        );
        assert_eq!(
            run(FilterOp::In, "_id", vec![Value::Int(1), Value::Int(2)]).len(),
            2
        );
        let raw = mongo::raw_filter("name: {$in: ['Ann', 'Cy']}, age: {$type: 'string'}").unwrap();
        assert_eq!(find_all(&f, raw), [Bson::String("x3".into())]);
    }

    #[test]
    fn grid_edits_apply_with_their_types_and_counts() {
        let f = people();
        let sample = f.docs("hr", "people");
        let cols = schema::columns(&schema::infer(&sample), sample.len());
        let mut loaded = LoadedTypes::default();
        for d in &sample {
            loaded.record(d);
        }
        let pk = |v: Value| RowKey::Pk(BTreeMap::from([("_id".to_string(), v)]));
        let changes = vec![
            // Bob's age is a long: it stays one.
            StagedChange::Update {
                row_id: "r2".into(),
                key: pk(Value::Int(2)),
                changes: BTreeMap::from([("age".to_string(), Value::Int(26))]),
                exprs: None,
            },
            StagedChange::Update {
                row_id: "r1".into(),
                key: pk(Value::Int(1)),
                changes: BTreeMap::from([("big".to_string(), values::missing())]),
                exprs: None,
            },
            StagedChange::Insert {
                row_id: "n1".into(),
                values: BTreeMap::from([("name".to_string(), Value::Text("Dee".into()))]),
                exprs: None,
            },
            StagedChange::Delete {
                row_id: "r3".into(),
                key: pk(Value::Text("x3".into())),
            },
            // Gone already: matches nothing, so its `n` is 0 and the commit reports it.
            StagedChange::Delete {
                row_id: "r9".into(),
                key: pk(Value::Int(9)),
            },
        ];
        let table = TableRef::new(Some("hr"), "people");
        let plan = mongo::plan_changes(&table, &cols, &loaded, &changes).unwrap();
        let ns: Vec<Option<i64>> = plan
            .ops
            .iter()
            .map(|op| catalog::reply_n(&f.run("hr", &op.command)))
            .collect();
        assert_eq!(ns, [Some(1), Some(1), Some(1), Some(1), Some(0)]);
        assert!(plan.ops.iter().all(|op| op.expect_n == 1));
        let docs = f.docs("hr", "people");
        assert_eq!(docs[1].get("age"), Some(&Bson::Int64(26)));
        assert!(!docs[0].contains_key("big"));
        assert!(matches!(docs[2].get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(docs.len(), 3);
    }

    #[test]
    fn typed_edits_and_replacements_apply_on_the_fake() {
        let f = FakeMongo::new();
        let oid = ObjectId::parse_str("65f1a2b3c4d5e6f708192a3b").unwrap();
        f.seed(
            "hr",
            "mixed",
            vec![
                doc! { "_id": 1, "ref": "a", "owner": oid },
                doc! { "_id": 2, "ref": oid, "owner": oid },
                doc! { "_id": 3, "ref": "c" },
            ],
        );
        let sample = f.docs("hr", "mixed");
        let cols = schema::columns(&schema::infer(&sample), sample.len());
        let mut loaded = LoadedTypes::default();
        for d in &sample {
            loaded.record(d);
        }
        let hex = "65f1a2b3c4d5e6f708192a3c";
        let new_oid = ObjectId::parse_str(hex).unwrap();
        let pk = |v: Value| RowKey::Pk(BTreeMap::from([("_id".to_string(), v)]));
        let changes = vec![
            // `ref` is a text column: the edited ObjectId cell comes as text and stays an
            // ObjectId.
            StagedChange::Update {
                row_id: "r2".into(),
                key: pk(Value::Int(2)),
                changes: BTreeMap::from([("ref".to_string(), Value::Text(hex.into()))]),
                exprs: None,
            },
            // Typed into the missing `owner` cell: the editor keeps the `missing` type.
            StagedChange::Update {
                row_id: "r3".into(),
                key: pk(Value::Int(3)),
                changes: BTreeMap::from([(
                    "owner".to_string(),
                    Value::other(values::TY_MISSING, hex),
                )]),
                exprs: None,
            },
        ];
        let table = TableRef::new(Some("hr"), "mixed");
        let plan = mongo::plan_changes(&table, &cols, &loaded, &changes).unwrap();
        for op in &plan.ops {
            assert_eq!(catalog::reply_n(&f.run("hr", &op.command)), Some(1));
        }
        let docs = f.docs("hr", "mixed");
        assert_eq!(docs[1].get("ref"), Some(&Bson::ObjectId(new_oid)));
        assert_eq!(docs[2].get("owner"), Some(&Bson::ObjectId(new_oid)));

        // A replacement matches the original whole, with the field names and values a plain
        // query would read as paths, operators and patterns.
        f.seed(
            "hr",
            "odd",
            vec![doc! {
                "_id": 1, "a.b": 1, "$x": { "$gt": 5 },
                "re": Regex { pattern: "^a".into(), options: "i".into() },
            }],
        );
        let original = f.docs("hr", "odd")[0].clone();
        let replace = mongo::replace_command("odd", &original, doc! { "_id": 1, "n": 2 });
        assert_eq!(catalog::reply_n(&f.run("hr", &replace)), Some(1));
        assert_eq!(f.docs("hr", "odd"), [doc! { "_id": 1, "n": 2 }]);
        // Changed since it was loaded: nothing matches, and the commit reports it.
        assert_eq!(catalog::reply_n(&f.run("hr", &replace)), Some(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn grid_pages_of_large_documents_come_whole() {
        let f = FakeMongo::new();
        let text = "x".repeat(2000);
        f.seed(
            "hr",
            "logs",
            (0..10)
                .map(|i| doc! { "_id": i, "text": text.clone() })
                .collect(),
        );
        // About two documents to a batch, as 16 MiB holds about 300 documents of 56 KB.
        f.batch_bytes(5000);
        let addr = f.serve().await;
        let client = ::mongodb::Client::with_uri_str(FakeMongo::uri(addr))
            .await
            .unwrap();
        let db = client.database("hr");
        // The find and its getMores in one session: a server reads a cursor on only in its own.
        let mut session = client.start_session().await.unwrap();
        let mut rows = Vec::new();
        for skip in [0_u64, 4, 8] {
            let spec = mongo::FindSpec {
                sort: doc! { "_id": 1 },
                skip,
                limit: 4,
                ..Default::default()
            };
            let mut page = mongo::PageRead::new("logs", &spec);
            let reply = db
                .run_command(mongo::find("logs", &spec))
                .session(&mut session)
                .await
                .unwrap();
            let (mut batch, mut cursor) = catalog::cursor_batch(&reply);
            loop {
                let step = page.take(&mut batch, cursor);
                rows.extend(batch.drain(..).filter_map(|d| d.get("_id").cloned()));
                match step {
                    mongo::PageStep::More(get_more) => {
                        if skip == 0 {
                            let other = db.run_command(get_more.clone()).await.unwrap_err();
                            assert!(matches!(
                                errors::from_driver(&other),
                                AppError::Db {
                                    vendor_code: Some(50737),
                                    ..
                                }
                            ));
                        }
                        let reply = db
                            .run_command(get_more)
                            .session(&mut session)
                            .await
                            .unwrap();
                        (batch, cursor) = catalog::cursor_batch(&reply);
                    }
                    mongo::PageStep::Close(kill) => {
                        db.run_command(kill).session(&mut session).await.unwrap();
                        break;
                    }
                    mongo::PageStep::Done => break,
                }
            }
        }
        // Every document once: no page came back short, so no skip passed over any.
        assert_eq!(rows, (0..10).map(Bson::Int32).collect::<Vec<_>>());
        // Two pages took a getMore each (and the first one a refused one outside its session).
        assert_eq!(f.commands_named("getMore").len(), 3);
        assert_eq!(f.open_cursors(), 0);
        drop(session);
        client.shutdown().await;
    }

    #[test]
    fn console_statements_run_on_the_fake() {
        let f = people();
        let run = |text: &str| {
            let shell::Statement::Run(c) = shell::parse(text, "hr").unwrap() else {
                panic!("{text}")
            };
            f.run(&c.db, &c.command)
        };
        let r = run("db.people.find({age: {$gte: 25}}).sort({age: -1}).limit(1)");
        assert_eq!(ids(&r), [Bson::Int32(1)]);
        assert_eq!(
            catalog::count_of(&run("db.people.countDocuments({name: /^[AB]/i})")),
            Some(2)
        );
        assert_eq!(catalog::count_of(&run("db.people.find().count()")), Some(3));
        assert_eq!(
            run("db.people.insertOne({name: 'Eve'})").get_i32("n"),
            Ok(1)
        );
        assert_eq!(
            run("db.people.updateMany({}, {$set: {seen: true}})").get_i64("n"),
            Ok(4)
        );
        assert_eq!(
            run("db.people.deleteOne({name: 'Eve'})").get_i64("n"),
            Ok(1)
        );
        assert_eq!(
            run("db.people.createIndex({name: 1}, {unique: true})").get_f64("ok"),
            Ok(1.0)
        );
        let dup = run("db.people.insertOne({name: 'Ann'})");
        assert!(matches!(
            errors::from_reply(&dup),
            Some(AppError::Db {
                vendor_code: Some(11000),
                ..
            })
        ));
        let idx = catalog::indexes(
            "people",
            &catalog::cursor_batch(&run("db.people.getIndexes()")).0,
        );
        assert_eq!(idx.len(), 2);
        assert!(idx[1].unique);
        let stats = catalog::cursor_batch(&run("db.people.stats()")).0;
        assert_eq!(catalog::coll_stats(&stats[0]).0, Some(3));
        let listed = catalog::cursor_batch(&run("show collections")).0;
        assert_eq!(catalog::collections(&listed, "hr").len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_driver_speaks_to_the_fake_server() {
        let f = people();
        let addr = f.serve().await;
        let client = ::mongodb::Client::with_uri_str(FakeMongo::uri(addr))
            .await
            .unwrap();
        let db = client.database("hr");
        db.run_command(mongo::ping()).await.unwrap();
        let info = db.run_command(mongo::build_info()).await.unwrap();
        assert_eq!(catalog::server_version(&info), "8.0.0-fake");
        let hello = db.run_command(mongo::hello()).await.unwrap();
        assert_eq!(
            catalog::Topology::from_hello(&hello),
            catalog::Topology::Standalone
        );

        // A grid page in one batch, then a cursor read over several (getMore).
        let spec = mongo::FindSpec {
            sort: doc! { "_id": 1 },
            limit: 2,
            ..Default::default()
        };
        let page = db.run_command(mongo::find("people", &spec)).await.unwrap();
        let (batch, cursor) = catalog::cursor_batch(&page);
        assert_eq!((batch.len(), cursor), (2, 0));
        let coll = db.collection::<Document>("people");
        let all: Vec<Document> = coll
            .find(doc! {})
            .batch_size(1)
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
        assert!(f.commands_named("getMore").len() >= 2);
        assert_eq!(f.open_cursors(), 0);

        // Writes through the driver's document sequences.
        coll.insert_many(vec![doc! { "name": "Fay" }, doc! { "name": "Gus" }])
            .await
            .unwrap();
        assert_eq!(coll.count_documents(doc! {}).await.unwrap(), 5);
        assert_eq!(db.list_collection_names().await.unwrap(), ["people"]);

        // A server error keeps its code; a killed operation is a cancel; maxTimeMS a timeout.
        let err = db.run_command(doc! { "frobnicate": 1 }).await.unwrap_err();
        assert!(matches!(
            errors::from_driver(&err),
            AppError::Db {
                vendor_code: Some(59),
                ..
            }
        ));

        f.delay("count", Duration::from_secs(5));
        let comment = mongo::op_comment("t1");
        let slow_db = db.clone();
        let slow_comment = comment.clone();
        let slow = tokio::spawn(async move {
            slow_db
                .run_command(doc! { "count": "people", "comment": slow_comment })
                .await
        });
        let admin = client.database("admin");
        let mut opid = None;
        for _ in 0..200 {
            let ops = admin
                .run_command(mongo::current_op_by_comment(&comment))
                .await
                .unwrap();
            if let Some(op) = catalog::cursor_batch(&ops).0.first() {
                opid = op.get("opid").cloned();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let opid = opid.expect("the slow count shows in $currentOp");
        admin.run_command(mongo::kill_op(opid)).await.unwrap();
        let killed = slow.await.unwrap().unwrap_err();
        assert!(matches!(errors::from_driver(&killed), AppError::Cancelled));

        let timed_out = db
            .run_command(doc! { "count": "people", "maxTimeMS": 50 })
            .await
            .unwrap_err();
        assert!(matches!(errors::from_driver(&timed_out), AppError::Timeout));
        client.shutdown().await;
    }
}
