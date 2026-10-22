# TodoMVC on Stardust.
#
# How the page stays up to date:
#
# - On load, the page opens one connection, GET /sse, and keeps it open.
#   The server sends the whole todo section down it straight away, then
#   again every time the todo list changes in the database. Datastar swaps
#   each copy into the page, changing only what differs.
#
# - Each click is a separate, short request: POST /, PATCH /{id},
#   DELETE /{id}, and so on. It changes the database and returns
#   204, with no HTML. The page doesn't update from that response; it
#   updates when the change arrives over the open connection, the same
#   way it does in every other tab viewing the list.
#
# Reads and writes travel on different requests. Datastar calls this CQRS.
#
#   http-nu --datastar --plugin ../../target/release/nu_plugin_stardust :3001 serve.nu

use http-nu/router *
use http-nu/datastar *

# Files are found next to this script, so it can be run or sourced from
# anywhere. Data goes in $env.CROSS_STREAM_SITE_STATE on cross.stream, which
# survives deploys, or next to the script otherwise.
const here = path self .
let state = $env.CROSS_STREAM_SITE_STATE? | default $here

# The plugin reads $env.STARDUST_LICENSE; load it from $state if it's there.
let license = $state | path join stardust.license
if $env.STARDUST_LICENSE? == null and ($license | path exists) {
  $env.STARDUST_LICENSE = open $license | str trim
}

let db = stardust open ($state | path join todos.db)

# This server's process id, and its plugin's: the child named
# nu_plugin_stard (/proc truncates names to 15 characters).
def self-pid []: nothing -> int {
  open --raw /proc/self/stat | into string | split row " " | first | into int
}
def plugin-pid []: nothing -> any {
  let me = self-pid
  ls /proc
  | where name =~ '^/proc/\d+$'
  | get name
  | each { try { open --raw ($in | path join stat) | into string | parse -r '^(?<pid>\d+) \((?<comm>.*)\) \S (?<ppid>\d+)' | first } catch { null } }
  | compact
  | where { ($in.ppid | into int) == $me and $in.comm == "nu_plugin_stard" }
  | get pid.0?
}

# If the plugin dies (out of memory, say), nu starts a new one, but $db
# belongs to the old one and every call on it fails from then on. Each
# request checks the plugin is still there; if it isn't, the server kills
# itself so its supervisor starts it again. SIGKILL, not exit: exit only
# ends the request, and a supervisor that restarts on failure counts SIGTERM
# as a clean stop.
let plugin = plugin-pid
# Passes its input, the request body, through.
def check-plugin [plugin] {
  if $plugin != null and not ($"/proc/($plugin)" | path exists) {
    print -e $"plugin ($plugin) is gone: restarting"
    kill --force (self-pid)
  }
  $in
}

# A todo is exactly these two fields. Queries select todos with
# [schema todo ?e], and mutations that do are checked against it.
stardust define $db schemas todo '{
  type                 object
  properties           {text {type string minLength 1} completed {type boolean}}
  required             [text completed]
  additionalProperties false
}' | ignore

# A null parameter default makes it required. completed can't be null:
# a mutation's patch is checked with the defaults filled in, and
# {completed null} would delete the field.

# The read model every viewer sees: all todos, oldest first.
stardust define $db queries todos/list '{
  find    [?e ?text ?completed]
  where   [[schema todo ?e] [?e text ?text] [?e completed ?completed]]
  orderBy [?e]
}' | ignore

# todos/list as it was at transaction ?at, for time travel. Stardust refuses
# an asOf parameter that defaults to null, so ?at defaults to 1, the first
# transaction; every caller passes it.
stardust define $db queries todos/list-at '{
  parameters {at 1}
  find       [?e ?text ?completed]
  where      [[schema todo ?e] [?e text ?text] [?e completed ?completed]]
  orderBy    [?e]
  bind       {with {db {asOf ?at}}}
}' | ignore

# The todo ?id, or nothing if ?id isn't a todo.
stardust define $db queries todos/get '{
  parameters {id null}
  find       [?e]
  where      [[schema todo ?e] [= ?e ?id]]
}' | ignore

# The completed todos.
stardust define $db queries todos/completed '{
  find  [?e]
  where [[schema todo ?e] [?e completed true]]
}' | ignore

# Every write below tags its transaction with the kind of change it made
# (#_tx is the transaction's own entity), so this lists the transactions that
# changed the list, oldest first: the steps of the time-travel slider.
stardust define $db queries todos/changes '{
  find    [?tx ?kind]
  where   [[?tx changes todos] [?tx kind ?kind]]
  orderBy [?tx]
}' | ignore

# The tag for a change of this kind: added, completed, reopened or deleted.
def tag [kind: string] { {'#_tx': {changes: todos kind: $kind}} }

# A todo's completed flag going to this value is this kind of change.
def flag-kind [completed: bool] { if $completed { "completed" } else { "reopened" } }

# The two mutations are built here and run inline, not stored: a tag can't
# be a parameter, and Stardust won't store a mutation whose patch has #_tx.

# Set the todo ?id's completed flag. no_match if ?id isn't a todo.
def set-one [kind: string] {
  {
    query: {
      parameters: {id: null completed: true}
      find:       ['?e']
      where:      [[schema todo '?e'] ['=' '?e' '?id']]
    }
    patch: ({'?e': {completed: '?completed'}} | merge (tag $kind))
  }
}

# Set completed on every todo that isn't already ?completed.
def set-all [kind: string] {
  {
    query: {
      parameters: {completed: true}
      find:       ['?e']
      where:      [[schema todo '?e'] ['?e' completed '?c'] ['!=' '?c' '?completed']]
    }
    patch: ({'?e': {completed: '?completed'}} | merge (tag $kind))
  }
}

def set-status [code: int] {
  metadata set { merge {'http.response': {status: $code}} }
}

def not-found [] { "Not Found" | set-status 404 }

# Run action on the input, unless it's empty.
def and-then [action: closure] {
  if ($in | is-not-empty) {
    $in | do $action
  }
}

# A route test for METHOD /7, matching any numeric id; whether 7 is a todo
# is up to the handler. Stardust refers to entity 7 as {#link: 7}, so that's
# what goes in ctx.id for the queries and mutations above.
def match-id-route [method: string] {
  {|req|
    if $req.method == $method {
      $req
      | path-matches "/:id"
      | and-then { try { $in.id | into int } }
      | and-then { {id: {'#link': $in}} }
    }
  }
}

# A patch that deletes each linked entity: [{#link: 7}] -> {"7": null}.
def delete-patch []: list -> record {
  each { {($in.'#link' | into string): null} } | into record | merge (tag deleted)
}

# todos/changes as {tx: 7, kind: added} rows.
def changes [db] {
  stardust query $db todos/changes | update tx { $in.'#link' }
}

# How the list changed from before to after, as {kind, todos} rows: added,
# deleted, completed and reopened, the ones that happened. Empty if it didn't.
def diff-lists [before: list, after: list]: nothing -> list {
  let flipped = $after | where {|t| $t.e in $before.e? and $t not-in $before }
  [
    {kind: added todos: ($after | where e not-in $before.e?)}
    {kind: deleted todos: ($before | where e not-in $after.e?)}
    {kind: completed todos: ($flipped | where completed)}
    {kind: reopened todos: ($flipped | where not completed)}
  ]
  | where { $in.todos | is-not-empty }
}

# What one change did, as the history header says it: Added "milk",
# Completed 3 todos, ...
def describe-change [before: list, after: list]: nothing -> string {
  diff-lists $before $after
  | each {|it|
    let verb = $it.kind | str capitalize
    if ($it.todos | length) == 1 { $'($verb) "($it.todos.0.text)"' } else { $"($verb) ($it.todos | length) todos" }
  }
  | str join ", "
}

# The history from before writes were tagged, as {tx, kind} rows like
# todos/changes. No query lists it, so read the list as of each old
# transaction, and keep the ones where it changed. That history can't change,
# so this runs once, at startup. It ends where tagging starts, or at the
# newest transaction if nothing is tagged yet.
let legacy = changes $db
  | get tx.0?
  | default (stardust subscribe $db todos/list | first | get transaction | $in + 1)
  | if $in <= 1 { [] } else {
    1..($in - 1)
    | each {|at| {tx: $at result: (stardust query $db todos/list-at --params {at: $at})} }
    | prepend {tx: 0 result: []}
    | window 2
    | each {|pair| {tx: $pair.1.tx kind: (diff-lists $pair.0.result $pair.1.result | get kind.0?)} }
    | where kind != null
  }

# Cache-buster for static/: a hash of its files, so a URL with ?v=<rev>
# changes exactly when a file does.
let rev = glob ($here | path join static "*")
  | sort
  | each { open --raw $in | hash md5 }
  | str join
  | hash md5
  | str substring 0..7

let index = .mj compile ($here | path join templates/index.html)
let section = .mj compile ($here | path join templates/todos.html)

# Notes on Stardust, one page per release: notes/<release>/index.md, and the
# files it links to beside it. Pages end in /, so relative links work the
# same here as on GitHub. A note on a topic rather than a release has the
# same layout, with the release it applies to in its frontmatter (release:
# alpha-2610212237).
let note = .mj compile ($here | path join templates/note.html)
# Highlighted code: OneHalf, dark or light to match the page.
let code_css = $"html.dark {(.highlight theme OneHalfDark)}\nhtml:not\(.dark) {(.highlight theme OneHalfLight)}"
let notes_dir = $here | path join notes

# Every note: releases and topics, by name.
def notes [dir: string]: nothing -> list<record> {
  ls $dir | where type == dir | get name | each {|d|
    let meta = open --raw ($d | path join index.md) | decode utf-8 | from md | where element == yaml | get content.0? | default "{}" | from yaml
    {name: ($d | path basename) release: $meta.release?}
  }
}

# The releases with notes, newest first.
def releases [dir: string]: nothing -> list<string> {
  notes $dir | where release == null | get name | sort --reverse
}

# The topic notes, by name.
def topics [dir: string]: nothing -> list<string> {
  notes $dir | where release != null | get name | sort
}

# A notes page, read with from md: meta, its frontmatter; title, its first
# heading; intro, the markdown before the first section; and blocks, one per
# second- or third-level heading, with its title, id ({#id} at the end of
# the heading), level, and markdown, from the heading to the next one.
# meta.code maps a file beside the page to the id of the block it ends, e.g.
# {leak.c: per-thread}.
def note-read []: string -> record {
  let markdown = $in
  let rows = $markdown | from md
  let lines = $markdown | lines
  let heads = $rows | where element == text and ($it.attributes?.level? | default 0) in [2 3]
  let starts = $heads | get content_span.start.line | each { $in - 1 }
  let body = $rows | where element == yaml | get content_span.end.line.0? | default 0
  {
    meta: ($rows | where element == yaml | get content.0? | default "{}" | from yaml)
    title: ($rows | where element == text and ($it.attributes?.level? | default 0) == 1 | get content.0? | default "notes")
    intro: ($lines | slice $body..<($starts.0? | default ($lines | length)) | str join "\n")
    blocks: (
      $heads | enumerate | each {|h|
        let start = $starts | get $h.index
        let end = $starts | skip ($h.index + 1) | get 0? | default ($lines | length)
        let parts = $h.item.content | split row " {#"
        {
          title: $parts.0
          id: ($parts.1? | default "" | str trim --right --char "}")
          level: $h.item.attributes.level
          markdown: ($lines | slice $start..<$end | str join "\n")
        }
      }
    )
  }
}

# A page's code files, as contents entries: {title, id, file: true}, for the
# blocks with these ids.
def note-code [code: record, ids: list<string>]: nothing -> list<record> {
  $code
  | transpose file id
  | where id in $ids
  | each {|c| {title: $c.file id: ($c.file | str replace -a '.' '-') file: true} }
}

# A page's sections for the contents: its second-level blocks, each with
# subs, the third-level blocks under it and the code that ends any of them.
def note-sections []: record -> list<record> {
  let page = $in
  let code = $page.meta.code? | default {}
  let blocks = $page.blocks | enumerate
  let tops = $blocks | where item.level == 2
  $tops | enumerate | each {|t|
    let start = $t.item.index
    let end = $tops | skip ($t.index + 1) | get 0?.index | default ($blocks | length)
    let under = $blocks | where index > $start and index < $end | get item
    {
      title: $t.item.item.title
      id: $t.item.item.id
      subs: (
        $under
        | each {|b| {title: $b.title id: $b.id file: false} }
        | append (note-code $code ([$t.item.item.id] ++ $under.id))
      )
    }
  }
}

# The contents for a page: the topics, then every release, newest first,
# each with the sections of the one being read (current).
def note-contents [dir: string, current: string]: nothing -> record {
  let entry = {|n|
    let sections = if $n == $current {
      open --raw ($dir | path join $n index.md) | decode utf-8 | note-read | note-sections
    } else { [] }
    {name: $n current: ($n == $current) sections: $sections}
  }
  {
    topics: (topics $dir | each {|n| do $entry $n })
    releases: (releases $dir | each {|n| do $entry $n })
  }
}

const copy_icon = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>'

# A file in dir, as an open block of highlighted code, its id the file name
# with . as - (leak.c is #leak-c). from is the release the file comes from
# when it isn't this page's, which the summary links to.
def code-file [dir: string, file: string, from?: string]: nothing -> string {
  let code = open --raw ($dir | path join $file) | decode utf-8
  let lang = $file | path parse | get extension
  let lines = $code | lines | length
  let body = $code | .highlight $lang | get __html
  let id = $file | str replace -a '.' '-'
  let same = if $from == null { "" } else { $", the same as in <a href=\"../($from)/#($id)\">($from)</a>" }
  $"<details open id=\"($id)\"><summary><code>($file)</code>, ($lines) lines($same)</summary>\n<pre><code class=\"language-($lang)\">($body)</code></pre>\n</details>"
}

# A page read by note-read, as HTML: each part through .md, each block
# followed by the code meta.code ends it with, and a copy button on every
# code block. The code is beside the page, or in meta.code_from, a release
# directory relative to it.
def note-html [dir: string]: record -> string {
  let page = $in
  let code = $page.meta.code? | default {} | transpose file id
  let from = $page.meta.code_from?
  let code_dir = if $from == null { $dir } else { $dir | path join $from | path expand }
  let release = if $from == null { null } else { $code_dir | path basename }
  $page.blocks
  | each {|b|
    $code
    | where id == $b.id
    | get file
    | each { code-file $code_dir $in $release }
    | prepend ($b.markdown | .md | get __html)
    | str join "\n"
  }
  | prepend ($page.intro | .md | get __html)
  | str join "\n"
  | str replace -a '<pre>' $'<figure><button type="button" aria-label="copy">($copy_icon)</button><pre>'
  | str replace -a '</pre>' '</pre></figure>'
}

# A notes page, with the notes contents beside it. dir holds the
# page and the files its meta.code names; url is the page's own, absolute,
# for its og tags. Its og image is og.png beside it, if there is one, and its
# description meta.description.
def note-page [note: any, code_css: string, rev: string, contents: record, dir: string, url: string]: string -> string {
  let page = note-read
  {
    title: $page.title
    description: ($page.meta.description? | default "")
    url: $url
    image: (if ($dir | path join og.png | path exists) { $"($url)og.png?v=($rev)" })
    body: ($page | note-html $dir)
    code: $code_css
    rev: $rev
    topics: $contents.topics
    releases: $contents.releases
  }
  | .mj render $note
}

# This site's URL, for og tags, which must be absolute: https, unless this is
# a local run. X-Forwarded-Proto can't be trusted for this: behind a chain of
# proxies the last hop may be plain http.
def site-url [req: record]: nothing -> string {
  let host = $req.headers.host
  let scheme = if $host =~ '^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$' { "http" } else { "https" }
  $"($scheme)://($host)/"
}

def redirect [to: string] {
  null | metadata set { merge {'http.response': {status: 302 headers: {Location: $to}}} }
}

# A route test for /notes paths: /notes, /notes/, /notes/<note>, /notes/<note>/
# and /notes/<note>/<file>, for a note (a release or a topic) and file that
# exist. ctx has release, the note, and file, either of which may be empty,
# and slash, whether a note came with its trailing /.
def match-notes-route [dir: string] {
  {|req|
    if $req.method == GET {
      $req.path
      | parse -r '^/notes(?:/(?<release>[^/]*)(?<slash>/?)(?<file>[^/]*))?$'
      | get 0?
      | and-then {
        let m = $in
        let release = $m.release? | default ""
        let file = $m.file? | default ""
        let known = $release == "" or ($release in (notes $dir | get name))
        let found = $file == "" or ($known and ($file in (ls ($dir | path join $release) | get name | path basename)))
        if $known and $found and ($file == "" or $file != "index.md") {
          {release: $release file: $file slash: ($m.slash? == "/")}
        }
      }
    }
  }
}

{|req|
  check-plugin $plugin
  | dispatch $req [
    (
      # $at 0 is now: stream every change, and send the slider its steps. Any other $at renders that change once, read-only,
      # with what it did.
      route {method: GET path: "/sse"} {|req ctx|
        from datastar-signals $req
        | get at? | default 0
        | if $in == 0 {
          stardust subscribe $db todos/list
          | each {|event|
            let changes = $legacy | append (changes $db)
            [
              ($event | insert live true | .mj render $section | to datastar-patch-elements)
              ({_steps: $changes.tx?} | to datastar-patch-signals)
            ]
          }
          | flatten
        } else {
          let at = $in
          let changes = $legacy | append (changes $db)
          let before = $changes | where tx < $at | last | get tx?
          let after = stardust query $db todos/list-at --params {at: $at}
          {
            result: $after
            transaction: $at
            live: false
            step: ($changes | where tx <= $at | length)
            steps: ($changes | length)
            kind: ($changes | where tx == $at | get kind.0?)
            change: (describe-change (if $before == null { [] } else { stardust query $db todos/list-at --params {at: $before} }) $after)
          }
          | .mj render $section
          | to datastar-patch-elements
        }
        | to sse
      }
    )

    (
      route {method: POST path: "/"} {|req ctx|
        from datastar-signals $req
        | get input? | default "" | str trim
        | if ($in | is-empty) {
          "Empty todo text" | set-status 400
        } else {
          {'#_todo': {text: $in completed: false}} | merge (tag added) | stardust patch $db | ignore
        }
      }
    )

    # The body is the state to set, {completed: true}, not a toggle. A request
    # that arrives twice (a double click, a retry) ends in the same state.
    (
      route {method: PATCH path: "/"} {|req ctx|
        from datastar-signals $req
        | select completed
        | stardust mutate $db (set-all (flag-kind $in.completed)) --params $in
        | ignore
      }
    )

    (
      route (match-id-route PATCH) {|req ctx|
        from datastar-signals $req
        | {id: $ctx.id completed: $in.completed}
        | stardust mutate $db (set-one (flag-kind $in.completed)) --params $in
        | if $in.status == "no_match" { not-found }
      }
    )

    (
      route {method: DELETE path: "/completed"} {|req ctx|
        # With nothing completed the patch would be only its tag, which
        # Stardust refuses.
        stardust query $db todos/completed
        | get e
        | and-then { delete-patch | stardust patch $db | ignore }
      }
    )

    # Mutations can't delete, so this is a check, then a patch, in two steps.
    # Deleting a todo that another request deleted in between is harmless.
    (
      route (match-id-route DELETE) {|req ctx|
        stardust query $db todos/get --params {id: $ctx.id}
        | if ($in | is-empty) {
          not-found
        } else {
          get e | delete-patch | stardust patch $db | ignore
        }
      }
    )

    (
      route {method: GET path: "/"} {|req ctx|
        {datastar: $DATASTAR_JS_PATH url: (site-url $req) rev: $rev} | .mj render $index
      }
    )

    (route {method: GET path-matches: "/static/:file"} {|req ctx| .static $here $req.path })

    (
      route (match-notes-route $notes_dir) {|req ctx|
        if $ctx.release == "" {
          redirect $"/notes/(releases $notes_dir | first)/"
        } else if not $ctx.slash {
          redirect $"/notes/($ctx.release)/"
        } else if $ctx.file == "" {
          open --raw ($notes_dir | path join $ctx.release index.md)
          | decode utf-8
          | note-page $note $code_css $rev (note-contents $notes_dir $ctx.release) ($notes_dir | path join $ctx.release) $"(site-url $req)notes/($ctx.release)/"
        } else if ($ctx.file | str ends-with ".png") {
          .static $here $req.path
        } else {
          open --raw ($notes_dir | path join $ctx.release $ctx.file)
          | decode utf-8
          | metadata set { merge {'http.response': {headers: {"Content-Type": "text/plain; charset=utf-8"}}} }
        }
      }
    )

    (route true {|req ctx| not-found })
  ]
}
