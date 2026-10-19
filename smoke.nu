# Smoke test: drives the plugin with the release's shared example data and
# checks each result against the file's `expected` values.
#
#   nu --plugin-config <registry> smoke.nu

use std/assert

const data = path self ../stardust-alpha-2610171653/abi/examples/data

def main [] {
    print $"license: (stardust license | select active failure name license expires)"

    # from dust, through `open`
    let people = open $"($data)/people.dust"
    let live = open $"($data)/live.dust"
    let expression = open $"($data)/expression.dust"

    # expressions, no database
    for i in 0..<($expression.inputs | length) {
        let got = stardust eval $expression.expression --input ($expression.inputs | get $i)
        assert equal $got ($expression.expected | get $i) $"expression input ($i)"
    }
    assert equal (stardust eval '[* ?n 2]' --input {n: 21}) 42
    print "eval: ok"

    let dir = mktemp -d
    let db = stardust open $"($dir)/people.db"

    let seeded = $people.seed | stardust patch $db
    assert equal $seeded.status committed
    assert equal ($seeded.ids | columns | sort) ['#_ada' '#_bob' '#_cy']
    print $"patch: ok, transaction ($seeded.transaction), ($seeded.facts) facts"

    for name in [names engineers] {
        let q = $people.queries | get $name
        let params = $q.parameters? | default {}
        let rows = stardust query $db $q.query --params $params --page-size ($q.pageSize? | default 0)
        assert equal $rows $q.expected $"query ($name)"
    }
    # a stream stops early: the first page only
    assert equal (stardust query $db $people.queries.names.query --page-size 1 | first 1) [{name: Ada}]
    print "query: ok"

    let first = stardust mutate $db $people.mutations.birthday
    let again = stardust mutate $db $people.mutations.birthday
    print $"mutate: ($first.status), then ($again.status)"
    assert equal (stardust query $db $people.queries.adaAge.query) $people.queries.adaAge.expected

    # stored definitions
    stardust define $db queries people/names $people.queries.names.query | ignore
    assert equal (stardust definitions $db queries) [people/names]
    assert equal (stardust query $db people/names) $people.queries.names.expected
    stardust define $db queries people/names null | ignore
    assert equal (stardust definitions $db queries) []
    print "definitions: ok"

    # documents as text: DUST by default, JSON with --format; a bare name is a path
    let names = $people.queries.names
    assert equal (stardust query $db ($names.query | to dust)) $names.expected
    assert equal (stardust query $db ($names.query | to json) --format json) $names.expected
    stardust define $db queries people/names ($names.query | to dust) | ignore
    assert equal (stardust query $db people/names) $names.expected
    assert equal (stardust query $db '"people/names"' --format json) $names.expected
    stardust define $db queries people/names null | ignore
    print "documents: ok"

    # live: commit from a background job while the subscription waits
    job spawn { sleep 500ms; $live.hire | stardust patch $db } | ignore
    let events = stardust subscribe $db $people.queries.names.query | first 2
    assert equal $events.0.result $people.queries.names.expected
    assert equal $events.1.result $live.hired
    print "subscribe: ok"

    # dates and durations round-trip through #utc / #dur
    let when = 2026-10-07T12:34:56Z
    let t = {'#_t': {at: $when, took: 1500ms}} | stardust patch $db
    let back = stardust query $db {find: ['?at' '?took'], where: [['?e' at '?at'] ['?e' took '?took']]}
    assert equal $back [{at: $when, took: 1500ms}]
    assert equal ({'#utc': '2026-10-07T12:34:56Z'} | to dust | from dust) $when
    print "time: ok"

    # formats: each round-trips nu values; tron is binary
    let v = {a: 1, b: [x 2.5 null true], c: {d: hi}, at: $when}
    assert equal ($v | to dust | from dust) $v
    assert equal ($v | to edn | from edn) $v
    assert equal ($v | to tron | describe) binary
    assert equal ($v | to tron | from tron) $v
    $v | save $"($dir)/v.edn"
    $v | save $"($dir)/v.tron"
    assert equal (open $"($dir)/v.edn") $v
    assert equal (open $"($dir)/v.tron") $v
    print "formats: ok"

    stardust close $db

    # plugins: listed without a database, called on one that enables them
    assert ("reactors" in (stardust plugins).name)
    let rdb = stardust open $"($dir)/reactors.db" --plugins {reactors: {}}
    assert equal (stardust plugin-call $rdb reactors status).enabled true
    stardust plugin-call $rdb reactors sync {maxWait: 1sec}
    assert error { stardust plugin-call $rdb backups status }
    stardust close $rdb
    print "plugins: ok"

    rm -rf $dir
    print "all ok"
}
