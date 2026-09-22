# Input: base index, then one runtime/platform entry per publication.
if length < 2 or (.[0] | type) != "object" then
  error("expected a base index and at least one fragment")
else . end
| .[0] as $base
| reduce .[1:][] as $fragment (
    {index: $base, seen: []};
    if ($fragment | type) != "object" or ($fragment | length) != 1 then
      error("fragment must contain exactly one runtime")
    else . end
    | ($fragment | keys[0]) as $runtime
    | if ($fragment[$runtime] | type) != "object" or ($fragment[$runtime] | length) != 1 then
        error("fragment must contain exactly one platform")
      else . end
    | ($fragment[$runtime] | keys[0]) as $platform
    | if ($fragment[$runtime][$platform] | type) != "object" then
        error("fragment entry must be an object")
      else . end
    | [$runtime, $platform] as $key
    | if any(.seen[]; . == $key) then
        error("duplicate publication for \($runtime)/\($platform)")
      else . end
    | .seen += [$key]
    # Replace the whole entry, including removal of obsolete optional fields.
    | .index[$runtime][$platform] = $fragment[$runtime][$platform]
  )
| .index
