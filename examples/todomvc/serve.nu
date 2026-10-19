# TodoMVC on Stardust. Writes are short requests that commit a patch; reads
# are one SSE stream per viewer, fed by a live query.
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

# The read model every viewer sees: all todos, oldest first.
stardust define $db queries todos/list '{
  find    [?e ?text ?completed]
  where   [[?e text ?text] [?e completed ?completed]]
  orderBy [?e]
}' | ignore

# One todo's completed flag. ?id is a link: {#link 7}.
stardust define $db queries todos/completed '{
  parameters {id 0}
  find       [?completed]
  where      [[?e completed ?completed] [= ?e ?id]]
}' | ignore

# Set completed on every todo that isn't already ?done.
stardust define $db mutations todos/set-all '{
  query {
    parameters {done true}
    find       [?e]
    where      [[?e completed ?c] [!= ?c ?done]]
  }
  patch {?e {completed ?done}}
}' | ignore

# A todo by the id in its URL, or nothing: only todos can be toggled or deleted.
let find = {|id|
  let id = try { $id | into int } catch { return null }
  stardust query $db todos/completed --params {id: {'#link': $id}} | get -o 0
}

let index = .mj compile ($here | path join templates/index.html)
let section = .mj compile ($here | path join templates/todos.html)

def not-found [] {
  "Not Found" | metadata set { merge {'http.response': {status: 404}} }
}

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
      route {method: GET path: "/todos/updates"} {|req ctx|
        stardust subscribe $db todos/list
        | each {|event| view $event | .mj render $section | to datastar-patch-elements }
        | to sse
      }
    )

    (
      route {method: POST path: "/todos/add"} {|req ctx|
        let text = $in | from datastar-signals $req | get -o input | default "" | str trim
        if ($text | is-empty) {
          return ("Empty todo text" | metadata set { merge {'http.response': {status: 400}} })
        }
        {'#_todo': {text: $text completed: false}} | stardust patch $db | ignore
      }
    )

    (
      route {method: POST path: "/todos/toggle-all"} {|req ctx|
        let rows = stardust query $db todos/list
        let done = $rows | any {|row| not $row.completed }
        stardust mutate $db todos/set-all --params {done: $done} | ignore
      }
    )

    (
      route {method: DELETE path: "/todos/completed"} {|req ctx|
        let ids = stardust query $db '{find [?e] where [[?e completed true]]}' | get e.'#link'
        if ($ids | is-not-empty) {
          $ids | reduce --fold {} {|id patch| $patch | insert ($id | into string) null } | stardust patch $db | ignore
        }
      }
    )

    (
      route {method: POST path-matches: "/todos/:id/toggle"} {|req ctx|
        let todo = do $find $ctx.id
        if $todo == null { return (not-found) }
        {($ctx.id): {completed: (not $todo.completed)}} | stardust patch $db | ignore
      }
    )

    (
      route {method: DELETE path-matches: "/todos/:id"} {|req ctx|
        if (do $find $ctx.id) == null { return (not-found) }
        {($ctx.id): null} | stardust patch $db | ignore
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

    (route {method: GET} {|req ctx| .static ($here | path join www) $req.path })

    (route true {|req ctx| not-found })
  ]
}
