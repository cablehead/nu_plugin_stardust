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

# The plugin reads $env.STARDUST_LICENSE; load it from state/ if it's there.
let license = $state | path join stardust.license
if $env.STARDUST_LICENSE? == null and ($license | path exists) {
  $env.STARDUST_LICENSE = open $license | str trim
}

let db = stardust open ($state | path join todos.db)

# A todo is exactly these two fields. Queries select todos with
# [schema todo ?e], and mutations that do are checked against it.
stardust define $db schemas todo '{
  type                 object
  properties           {text {type string minLength 1} completed {type boolean}}
  required             [text completed]
  additionalProperties false
}' | ignore

# The read model every viewer sees: all todos, oldest first.
stardust define $db queries todos/list '{
  find    [?e ?text ?completed]
  where   [[schema todo ?e] [?e text ?text] [?e completed ?completed]]
  orderBy [?e]
}' | ignore

# The todo ?id, or nothing if ?id isn't a todo.
stardust define $db queries todos/get '{
  parameters {id 0}
  find       [?e]
  where      [[schema todo ?e] [= ?e ?id]]
}' | ignore

# The completed todos.
stardust define $db queries todos/completed '{
  find  [?e]
  where [[schema todo ?e] [?e completed true]]
}' | ignore

# Set the todo ?id's completed flag. no_match if ?id isn't a todo.
stardust define $db mutations todos/set '{
  query {
    parameters {id 0 completed true}
    find       [?e]
    where      [[schema todo ?e] [= ?e ?id]]
  }
  patch {?e {completed ?completed}}
}' | ignore

# Set completed on every todo that isn't already ?completed.
stardust define $db mutations todos/set-all '{
  query {
    parameters {completed true}
    find       [?e]
    where      [[schema todo ?e] [?e completed ?c] [!= ?c ?completed]]
  }
  patch {?e {completed ?completed}}
}' | ignore

# A route test for METHOD /7. Stardust refers to entity 7 as {#link: 7},
# so that's what goes in ctx.id for the queries and mutations above.
def todo [method: string] {
  {|req|
    if $req.method == $method and $req.path =~ '^/\d+$' {
      {id: {'#link': ($req.path | str substring 1.. | into int)}}
    }
  }
}

let index = .mj compile ($here | path join templates/index.html)
let section = .mj compile ($here | path join templates/todos.html)

def status [code: int] {
  metadata set { merge {'http.response': {status: $code}} }
}

def not-found [] { "Not Found" | status 404 }

# Rows from todos/list, as the template's view model.
def view [event: record] {
  let todos = $event.result | each {|row|
    {id: $row.e.'#link' text: $row.text completed: $row.completed}
  }
  {
    todos: $todos
    active_count: ($todos | where not completed | length)
    completed_count: ($todos | where completed | length)
    transaction: $event.transaction
    sequence: $event.sequence
  }
}

{|req|
  dispatch $req [
    (
      route {method: GET path: "/sse"} {|req ctx|
        stardust subscribe $db todos/list
        | each {|event| view $event | .mj render $section | to datastar-patch-elements }
        | to sse
      }
    )

    (
      route {method: POST path: "/"} {|req ctx|
        from datastar-signals $req
        | get -o input | default "" | str trim
        | if ($in | is-empty) {
          "Empty todo text" | status 400
        } else {
          {'#_todo': {text: $in completed: false}} | stardust patch $db | ignore
        }
      }
    )

    # The body is the state to set, {completed: true}, so a repeat is harmless.
    (
      route {method: PATCH path: "/"} {|req ctx|
        from datastar-signals $req
        | select completed
        | stardust mutate $db todos/set-all --params $in
        | ignore
      }
    )

    (
      route (todo PATCH) {|req ctx|
        from datastar-signals $req
        | {id: $ctx.id completed: $in.completed}
        | stardust mutate $db todos/set --params $in
        | if $in.status == "no_match" { not-found }
      }
    )

    (
      route {method: DELETE path: "/completed"} {|req ctx|
        stardust query $db todos/completed
        | each { {($in.e.'#link' | into string): null} }
        | into record
        | stardust patch $db
        | ignore
      }
    )

    # A patch, not a mutation: mutations can't delete.
    (
      route (todo DELETE) {|req ctx|
        stardust query $db todos/get --params {id: $ctx.id}
        | if ($in | is-empty) {
          not-found
        } else {
          {($ctx.id.'#link' | into string): null} | stardust patch $db | ignore
        }
      }
    )

    (
      route {method: GET path: "/"} {|req ctx|
        # og:image must be absolute: https, unless this is a local run.
        # X-Forwarded-Proto can't be trusted for this: behind a chain of
        # proxies the last hop may be plain http.
        let host = $req.headers.host
        let local = $host =~ '^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$'
        let scheme = if $local { "http" } else { "https" }
        {datastar: $DATASTAR_JS_PATH url: $"($scheme)://($host)/"} | .mj render $index
      }
    )

    (
      route {|req| if $req.method == "GET" and ($req.path | str starts-with "/static/") { {} } } {|req ctx|
        .static $here $req.path
      }
    )

    (route true {|req ctx| not-found })
  ]
}
