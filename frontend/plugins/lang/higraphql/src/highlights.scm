; Hand-written minimal query — no upstream ships one for this grammar.
(comment) @comment
(string_value) @string
(int_value) @number
(float_value) @number
(variable) @variable
(named_type) @type
["true" "false"] @constant
["type" "enum" "union" "interface" "schema" "scalar" "input" "fragment"
 "query" "mutation" "subscription" "directive" "extend" "implements"
 "on"] @keyword
