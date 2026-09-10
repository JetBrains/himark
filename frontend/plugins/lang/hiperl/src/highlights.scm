; Hand-written minimal query — no upstream ships one for this grammar.
(comments) @comment
(string_single_quoted) @string
(string_double_quoted) @string
(string_q_quoted) @string
(string_qq_quoted) @string
(integer) @number
(scalar_variable) @variable
(array_variable) @variable
(hash_variable) @variable
["sub" "my" "our" "local" "state" "use" "no" "require" "return" "if"
 "else" "elsif" "unless" "while" "until" "for" "foreach" "last" "next"
 "redo" "goto" "package" "and" "or" "not" "bless"] @keyword
