;; Literals

(num_lit) @number

(char_lit) @string
(str_lit) @string

(kwd_lit) @constant

;; Comments

(comment) @comment

;; Quote markers

"'" @operator
"`" @operator
"," @operator

;; Unquote splicing
(unquote_splicing_lit) @operator
(unquoting_lit) @operator

;; Special keyword nodes for defun, defmacro, defmethod, defgeneric
(defun_keyword) @keyword

;; Loop keywords
(loop_macro "loop" @function.macro)

;; Match "for" within for_clause context
(for_clause "for" @operator)

;; Other loop clause words (from, to, below, etc)
(for_clause_word) @operator

;; Accumulation verbs (collect, sum, etc)
(accumulation_verb) @function.builtin

;; Loop control keywords
"into" @operator
"finally" @operator

;; Definition forms (defun, defmacro, defclass, defvar, etc.)
((sym_lit) @keyword
  (#match? @keyword
    "^(defun|defmacro|defgeneric|defmethod|defclass|defstruct|deftype|defvar|defparameter|defconstant|defpackage|defsetf|define-compiler-macro|define-condition|define-symbol-macro|define-method-combination|define-setf-expander|define-modify-macro)$"
    ))

;; Local binding forms (let, lambda, flet, etc.)
((sym_lit) @function.macro
  (#match? @function.macro
    "^(let|let\\*|lambda|flet|labels|macrolet|symbol-macrolet|multiple-value-bind|destructuring-bind)$"
    ))

;; Exception handling and assertions
((sym_lit) @keyword.conditional
  (#match? @keyword.conditional
    "^(handler-case|handler-bind|restart-case|restart-bind|ignore-errors|check-type|assert)$"
    ))

;; Resource management (with-* macros)
((sym_lit) @function.builtin
  (#match? @function.builtin
    "^(with-slots|with-accessors|with-open-file|with-output-to-string|with-input-from-string|with-standard-io-syntax)$"
    ))

;; Type and declaration forms
((sym_lit) @type
  (#match? @type
    "^(the|declare|locally|eval-when|load-time-value)$"
    ))

;; Package operations
((sym_lit) @namespace
  (#match? @namespace
    "^(defpackage|in-package|import|export|use-package|require|provide)$"
    ))

;; Assignment, quote, and logic operators
((sym_lit) @operator
  (#match? @operator
    "^(setf|setq|quote|function|and|or|not)$"
    ))

;; Multiple values operations
((sym_lit) @function.builtin
  (#match? @function.builtin
    "^(values|multiple-value-call|multiple-value-list)$"
    ))

;; Error signaling
((sym_lit) @function.builtin
  (#match? @function.builtin
    "^(error|warn|cerror)$"
    ))

;; Built-in constants
(nil_lit) @constant.builtin

((sym_lit) @constant.builtin
  (#match? @constant.builtin "^t$"))

;; Lambda list keywords (&optional, &rest, &key, &whole, etc.)
((sym_lit) @type
 (#match? @type "^&"))

;; Operators
((sym_lit) @operator
  (#match? @operator
    "^(\\+|-|\\*|/|=|/=|<|>|<=|>=|eq|eql|equal|equalp)$"
    ))

;; Built-in functions (including type coercion/checking functions, common type names, and control flow markers)
((sym_lit) @function.builtin
  (#match? @function.builtin
    "^(null|car|cdr|caar|cadr|cdar|cddr|cons|list|append|reverse|length|member|assoc|rassoc|nth|nthcdr|last|butlast|remove|remove-if|remove-if-not|delete|delete-if|delete-if-not|substitute|subst|mapcar|mapcan|mapc|maplist|reduce|some|every|notany|notevery|find|find-if|position|count|count-if|sort|merge|union|intersection|set-difference|subsetp|adjoin|pushnew|copy-list|copy-tree|copy-seq|make-list|make-array|aref|svref|array-dimension|array-dimensions|array|vector|vector-push|vector-pop|bit-vector|simple-vector|simple-array|simple-string|simple-bit-vector|make-hash-table|gethash|remhash|maphash|hash-table|hash-table-count|clrhash|print|prin1|princ|write|format|read|read-line|read-char|peek-char|unread-char|fresh-line|terpri|open|close|pathname|stream|with-open-file|get|put|remprop|symbol-name|symbol-package|symbol-value|symbol-function|symbol-plist|make-symbol|gensym|intern|symbol|keyword|string|character|integer|float|rational|complex|number|fixnum|bignum|atom|sequence|function|otherwise|string=|string-equal|char|char=|char-equal|upper-case-p|lower-case-p|alphanumericp|digit-char-p|characterp|stringp|numberp|integerp|floatp|rationalp|complexp|realp|symbolp|listp|consp|arrayp|vectorp|hash-table-p|streamp|pathnamep|functionp|keywordp|atom|packagep|zerop|plusp|minusp|oddp|evenp|abs|min|max|floor|ceiling|truncate|round|mod|rem|sin|cos|tan|asin|acos|atan|sinh|cosh|tanh|exp|expt|log|sqrt|random|1\\+|1-|incf|decf|boundp|fboundp|makunbound|fmakunbound|type-of|typep|subtypep|coerce|error|warn|cerror|signal|make-condition|invoke-restart|find-restart|compute-restarts|describe|inspect|documentation|apropos|apropos-list|disassemble|trace|untrace|step|time|get-universal-time|get-internal-real-time|sleep|make-instance|slot-value|slot-boundp|slot-makunbound|class-of|find-class|change-class|update-instance-for-different-class|initialize-instance|reinitialize-instance|shared-initialize)$"
    ))

;; IMPORTANT: These must come LAST to override earlier patterns

;; Conditionals
((sym_lit) @operator
  (#match? @operator
    "^(if|when|unless|cond|case|ecase|typecase|etypecase|ctypecase)$"
    ))

;; Iteration/loops
((sym_lit) @function.macro
  (#match? @function.macro
    "^(do|do\\*|dotimes|dolist)$"
    ))

;; Block control and returns
((sym_lit) @function.builtin
  (#match? @function.builtin
    "^(block|return|return-from|tagbody|go)$"
    ))

;; Exception handling
((sym_lit) @function.builtin
  (#match? @function.builtin
    "^(catch|throw|unwind-protect)$"
    ))

;; Sequencing constructs
((sym_lit) @function.macro
  (#match? @function.macro
    "^(progn|prog1|prog2|multiple-value-prog1|progv)$"
    ))
