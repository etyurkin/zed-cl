;;;; master-repl.lisp - Shared REPL server for all implementations
;;;;
;;;; This is the ONE persistent Lisp process that holds the actual REPL state.
;;;; All Jupyter kernels connect to this master REPL and share the same environment.
;;;;
;;;; Balanced refactoring: Focus on clarity while keeping functions small

(defpackage :zed-cl.master-repl
  (:use :cl)
  (:import-from :zed-cl.config
                #:*completion-package-whitelist*)
  (:import-from :zed-cl.compat
                #:get-lambda-list
                #:get-function-type
                #:get-source-location
                #:get-backtrace
                #:getenv
                #:system-package-p
                #:with-source-tracking)
  (:export #:start-master-repl))

(in-package :zed-cl.master-repl)

;;;; Global state

(defparameter *master-running* t
  "Is master REPL running?")


;;;; Initialize package whitelist
(unless (listp *completion-package-whitelist*)
  (setf *completion-package-whitelist*
        '("KEYWORD" "COMMON-LISP" "COMMON-LISP-USER" "ZED-CL")))

;;;; Protocol - I/O helpers

(defun log-error (format-string &rest args)
  "Log to error output"
  (apply #'format *error-output* 
         (concatenate 'string "~&[Master] " format-string "~%") args)
  (force-output *error-output*))

(defun write-message (stream message)
  "Write a message to stream as s-expression"
  (handler-case
      (progn
        (prin1 message stream)
        (write-char #\Newline stream)
        (force-output stream)
        t)
    (error (e) (log-error "Error writing message: ~A" e) nil)))

;;;; Symbol Classification

(defun user-package-p (pkg)
  "Check if package is a user package (not a system package)"
  (when pkg
    (let ((pkg-name (package-name pkg)))
      (not (or (zed-cl.compat:system-package-p pkg-name)
               (string= pkg-name "ZED-CL"))))))

(defun whitelisted-package-name-p (pkg-name)
  "Check if a package name (string) is whitelisted"
  (cond
    ((eq *completion-package-whitelist* :unset)
     (let ((pkg (find-package pkg-name)))
       (and pkg (user-package-p pkg))))
    ((listp *completion-package-whitelist*)
     (member pkg-name *completion-package-whitelist* :test #'string=))
    (t nil)))

(defun core-keyword-p (keyword-symbol)
  "Check if a keyword is a 'core' keyword (not from system packages)"
  (let ((name (symbol-name keyword-symbol)))
    (and (not (find #\/ name))
         (not (zed-cl.compat:system-package-p name))
         (not (find-package name)))))

(defun whitelisted-keyword-p (keyword-symbol)
  "Check if keywords should be included in completions"
  (cond
    ((eq *completion-package-whitelist* :unset) (core-keyword-p keyword-symbol))
    ((listp *completion-package-whitelist*)
     (cond
       ((member "ALL-KEYWORDS" *completion-package-whitelist* :test #'string=) t)
       ((member "CORE-KEYWORDS" *completion-package-whitelist* :test #'string=)
        (core-keyword-p keyword-symbol))
       (t nil)))
    (t t)))

(defun whitelisted-package-p (pkg)
  "Check if package should be included in completions"
  (let ((pkg-name (package-name pkg)))
    (or (string= pkg-name "KEYWORD")
        (string= pkg-name "ZED-CL")
        ;; Always include user-defined packages
        (user-package-p pkg)
        ;; Include system packages if in whitelist
        (member pkg-name *completion-package-whitelist* :test #'string=))))

;;;; Symbol Introspection

(defun make-param-type-pair (param type)
  "Create (param . type) pair, filtering out T and *"
  (cons (symbol-name param)
        (if (or (eq type 't) (eq type '*))
            nil
            (format nil "~A" type))))

(defun extract-param-types (symbol arg-types)
  "Extract parameter types from function type info"
  (let ((lambda-list (get-lambda-list symbol))
        (param-types nil))
    (when (consp arg-types)
      (loop for param in lambda-list
            for type in arg-types
            unless (member param lambda-list-keywords)
            do (push (make-param-type-pair param type) param-types)))
    (nreverse param-types)))

(defun get-parameter-types (symbol)
  "Get parameter type information for a function symbol"
  (when (fboundp symbol)
    (handler-case
        (let ((ftype (get-function-type symbol)))
          (when (and ftype (consp ftype) (eq (car ftype) 'function))
            (extract-param-types symbol (second ftype))))
      (error () nil))))

;;;; Symbol Collection

(defun symbol-kind (sym is-cl-package is-keyword-package)
  "Determine the kind of symbol"
  (cond ((special-operator-p sym) "special-operator")
        ((macro-function sym) "macro")
        ((fboundp sym) "function")
        ((boundp sym) "variable")
        (is-cl-package "keyword")
        (is-keyword-package "variable")
        (t nil)))

(defun get-symbol-source (sym)
  "Get source code representation for symbol"
  (handler-case
      (let ((def (get-lambda-list sym)))
        (format nil "(defun ~A ~A)" (string-downcase (symbol-name sym)) def))
    (error () nil)))

(defun should-collect-symbol-p (is-keyword-package is-user-package
                                 is-cl-package has-definition key seen sym)
  "Check if symbol should be included in collection"
  (and (not (gethash key seen))
       (or is-keyword-package is-user-package is-cl-package has-definition)
       (or (not is-keyword-package) (whitelisted-keyword-p sym))))

(defun remap-sbcl-source-path (path-string)
  "Remap SBCL source paths from build directory to installed location"
  (let ((sbcl-home (getenv "SBCL_HOME")))
    (when sbcl-home
      ;; If path contains /src/code/, /src/compiler/, etc., remap to installed location
      (let ((src-pos (search "/src/" path-string)))
        (when src-pos
          (let* ((relative-path (subseq path-string src-pos))
                 ;; Try both lib/sbcl/src and share/sbcl/src
                 (paths-to-try (list
                                (concatenate 'string sbcl-home relative-path)
                                (concatenate 'string
                                            (subseq sbcl-home 0 (search "/lib/sbcl" sbcl-home))
                                            "/share/sbcl" relative-path))))
            (dolist (try-path paths-to-try)
              (let ((probe-result (probe-file try-path)))
                (when probe-result
                  (return-from remap-sbcl-source-path (namestring probe-result))))))))))
  ;; Return original path if no remapping found
  path-string)

(defun get-source-location-for-symbol (sym)
  "Extract source file location for symbol (implementation-specific)"
  ;; Use compat layer for base introspection
  (let ((location (zed-cl.compat:get-source-location sym)))
    (when location
      ;; Apply SBCL-specific path remapping if needed
      (let ((path (first location))
            (line (second location))
            (char (third location)))
        (when path
          (list (remap-sbcl-source-path path) line char))))))

(defun collect-symbol-info (sym pkg is-keyword-package is-cl-package kind)
  "Create symbol info plist"
  (declare (ignore is-keyword-package is-cl-package))
  (let* ((source (when (or (fboundp sym) (special-operator-p sym))
                   (if (member kind '("function" "special-operator" "macro") :test #'string=)
                       (get-symbol-source sym)
                       nil)))
         (param-types (when (and (fboundp sym) (string= kind "function"))
                       (get-parameter-types sym)))
         (source-loc (get-source-location-for-symbol sym)))
    (when kind
      (append (list :symbol (symbol-name sym)
                   :kind kind
                   :package (package-name pkg)
                   :source source)
              (when param-types (list :param-types param-types))
              (when source-loc
                (list :source-file (first source-loc)
                      :source-line (second source-loc)
                      :source-character (third source-loc)))))))

(defun process-symbol (sym pkg seen)
  "Process a symbol and return its info if it should be collected"
  (let* ((pkg-name (package-name pkg))
         (is-keyword-package (string= pkg-name "KEYWORD"))
         (is-cl-package (string= pkg-name "COMMON-LISP"))
         (is-user-package (user-package-p pkg))
         (has-definition (or (fboundp sym) (boundp sym) (special-operator-p sym)))
         (key (cons pkg-name (symbol-name sym))))
    (when (should-collect-symbol-p is-keyword-package is-user-package
                                    is-cl-package has-definition key seen sym)
      (setf (gethash key seen) t)
      (collect-symbol-info sym pkg is-keyword-package is-cl-package
                          (symbol-kind sym is-cl-package is-keyword-package)))))

(defun collect-user-symbols ()
  "Collect all symbols from whitelisted packages"
  (log-error "Whitelist: ~S" *completion-package-whitelist*)
  (let ((symbols nil)
        (seen (make-hash-table :test 'equal)))
    (do-all-symbols (sym)
      (let ((pkg (symbol-package sym)))
        (when (and pkg
                   (eq (symbol-package sym) pkg)
                   (whitelisted-package-p pkg))
          (let ((info (process-symbol sym pkg seen)))
            (when info (push info symbols))))))
    (log-error "Collected ~D symbols" (length symbols))
    symbols))

;;;; Package Information

(defun count-exported-symbols (pkg)
  "Count exported symbols in package"
  (let ((count 0))
    (do-external-symbols (sym pkg) (incf count))
    count))

(defun build-package-doc (nicknames pkg-doc symbol-count)
  "Build documentation string from package metadata"
  (let ((parts nil))
    (when nicknames
      (push (format nil "Nicknames: ~{~A~^, ~}" nicknames) parts))
    (when pkg-doc
      (push (if parts (format nil "~%~A" pkg-doc) pkg-doc) parts))
    (push (if parts
              (format nil "~%~%Exported symbols: ~D" symbol-count)
              (format nil "Exported symbols: ~D" symbol-count))
          parts)
    (format nil "~{~A~^~%~}" (nreverse parts))))

(defun get-package-info (package-name)
  "Get information about a package, including metadata and exported symbol count"
  (let ((pkg (find-package (string-upcase package-name))))
    (when pkg
      (let ((nicknames (package-nicknames pkg))
            (pkg-doc (documentation pkg t))
            (symbol-count (count-exported-symbols pkg)))
        (list :symbol package-name
              :kind "package"
              :package package-name
              :doc (build-package-doc nicknames pkg-doc symbol-count)
              :source (format nil "(in-package :~A)" (string-downcase package-name)))))))

;;;; Symbol Information

(defun find-symbol-in-user-packages (symbol-name)
  "Find symbol in user-defined packages"
  (block found
    (do-all-symbols (s)
      (when (and (string= (symbol-name s) (string-upcase symbol-name))
                 (symbol-package s)
                 (user-package-p (symbol-package s))
                 (eq (symbol-package s) (symbol-package s)))
        (return-from found s)))
    nil))

(defun find-symbol-by-name (symbol-name package-name)
  "Find symbol by name, optionally in specific package"
  (if package-name
      (find-symbol (string-upcase symbol-name) 
                  (find-package (string-upcase package-name)))
      (or (find-symbol (string-upcase symbol-name) :cl)
          (find-symbol-in-user-packages symbol-name))))

(defun get-symbol-kind (sym)
  "Get symbol kind"
  (cond ((special-operator-p sym) "special-operator")
        ((macro-function sym) "macro")
        ((fboundp sym) "function")
        ((boundp sym) "variable")
        (t nil)))

(defun get-symbol-doc (sym kind)
  "Get documentation for symbol"
  (when kind
    (or (documentation sym 'function)
        (documentation sym 'variable)
        nil)))

(defun get-symbol-source-info (sym kind symbol-name)
  "Get source information for symbol"
  (handler-case
      (cond
        ((string= kind "macro")
         (let ((def (get-lambda-list sym)))
           (format nil "(defmacro ~A ~A)" (string-downcase symbol-name) def)))
        ((string= kind "function")
         (let ((def (get-lambda-list sym)))
           (format nil "(defun ~A ~A)" (string-downcase symbol-name) def)))
        ((string= kind "special-operator")
         (format nil "(special-operator ~A)" (string-downcase symbol-name)))
        (t nil))
    (error () nil)))

(defun build-symbol-info (sym symbol-name &optional package-name)
  "Build complete symbol information"
  (declare (ignore package-name))
  (let* ((pkg (symbol-package sym))
         (kind (get-symbol-kind sym))
         (doc (get-symbol-doc sym kind))
         (source (get-symbol-source-info sym kind symbol-name))
         (param-types (when (and (fboundp sym) (string= kind "function"))
                       (get-parameter-types sym)))
         (source-loc (get-source-location-for-symbol sym)))
    (when kind
      (append (list :symbol symbol-name
                   :kind kind
                   :package (package-name pkg)
                   :doc doc
                   :source (or source
                              (format nil "(~A ~A ...)"
                                     (cond ((string= kind "function") "defun")
                                           ((string= kind "macro") "defmacro")
                                           ((string= kind "special-operator") "special-operator")
                                           ((string= kind "variable") "defvar")
                                           (t "def"))
                                     (string-downcase symbol-name))))
              (when param-types (list :param-types param-types))
              (when source-loc
                (list :source-file (first source-loc)
                      :source-line (second source-loc)
                      :source-character (third source-loc)))))))

(defun get-symbol-info (symbol-name &optional package-name)
  "Get detailed information about a symbol including source and documentation"
  (let ((as-package (find-package (string-upcase symbol-name))))
    (when (and as-package (not package-name))
      (return-from get-symbol-info (get-package-info symbol-name))))
  (let ((sym (find-symbol-by-name symbol-name package-name)))
    (when (and sym (symbol-package sym))
      (build-symbol-info sym symbol-name package-name))))

;;;; Package Whitelist Management

(defun update-package-whitelist ()
  "Add any new user-defined packages to the whitelist (only if whitelist is :unset)"
  ;; Only auto-update if whitelist is :unset (not explicitly configured)
  (when (eq *completion-package-whitelist* :unset)
    (dolist (pkg (list-all-packages))
      (let ((pkg-name (package-name pkg)))
        (when (user-package-p pkg)
          (pushnew pkg-name *completion-package-whitelist* :test #'string=))))))

;;;; Code Evaluation

(defun clear-display-outputs ()
  "Clear any previous display outputs"
  (when (fboundp 'cl-user::clear-display-outputs)
    (funcall 'cl-user::clear-display-outputs)))

(defun collect-display-outputs ()
  "Collect any display outputs that were queued"
  (when (fboundp 'zed-cl::get-display-outputs)
    (funcall 'zed-cl::get-display-outputs)))


(defun eval-forms-from-code (code &optional file-path line character)
  "Read and evaluate all forms from code string"
  (declare (ignore line character))
  (let ((values nil))
    (with-input-from-string (stream code)
      (loop for form = (read stream nil :eof)
            until (eq form :eof)
            do (setf values
                     (if file-path
                         ;; Compile with source tracking for better backtraces
                         (with-source-tracking (file-path)
                           (multiple-value-list
                            (funcall (compile nil `(lambda () ,form)))))
                         ;; Regular eval without source tracking
                         (multiple-value-list (eval form))))))
    values))

(defun capture-backtrace ()
  "Capture current backtrace as string"
  (get-backtrace))

(defun eval-with-output-capture (code &optional file-path line character)
  "Evaluate code with output capture, return (values error displays)"
  (let ((output (make-string-output-stream)))
    (handler-case
        (let ((*standard-output* output)
              (*error-output* output)
              (*trace-output* output)
              #+sbcl (sb-ext:*muffled-warnings* nil))
          (clear-display-outputs)
          (let ((values (eval-forms-from-code code file-path line character)))
            (update-package-whitelist)
            (list values (get-output-stream-string output) nil nil
                  (collect-display-outputs))))
      (end-of-file ()
        (list nil (get-output-stream-string output)
              "Incomplete code: unbalanced parentheses"
              (list "Incomplete code") nil))
      (error (e)
        (list nil (get-output-stream-string output)
              (format nil "~A" e)
              (capture-backtrace) nil)))))

(defun eval-code (code &optional file-path line character)
  "Evaluate code in the master REPL, return (output values error traceback displays)"
  (destructuring-bind (values output error traceback displays)
      (eval-with-output-capture code file-path line character)
    (list :output output
          :values values
          :error error
          :traceback traceback
          :displays displays)))

;;;; Message Handlers

(defun build-eval-response (msg-id result)
  "Build response for eval request"
  (let ((response (list :id msg-id
                       :output (getf result :output)
                       :values (mapcar #'prin1-to-string
                                      (remove nil (getf result :values)))
                       :error (getf result :error)
                       :traceback (getf result :traceback))))
    (when (getf result :displays)
      (setf response (append response (list :displays (getf result :displays)))))
    response))

(defun handle-eval-message (msg-id code file-path file-line file-char stream)
  "Handle eval message type"
  (log-error "Eval request: ~A from ~A:~A:~A"
             (subseq code 0 (min 50 (length code)))
             (or file-path "interactive")
             (or file-line 0)
             (or file-char 0))
  (let ((result (eval-code code file-path file-line file-char)))
    (write-message stream (build-eval-response msg-id result))))

(defun handle-ping-message (msg-id stream)
  "Handle ping message type"
  (write-message stream (list :id msg-id :pong t)))

(defun filter-by-exact-package (prefix-upper all-symbols)
  "Filter symbols by exact package match"
  (let ((exact-pkg (find-package prefix-upper)))
    (when exact-pkg
      (remove-if-not
       (lambda (sym)
         (string= (getf sym :package) (package-name exact-pkg)))
       all-symbols))))

(defun filter-by-partial-package (prefix-upper all-symbols)
  "Filter symbols by partial package name match"
  (remove-if-not
   (lambda (sym)
     (let ((pkg-name (getf sym :package)))
       (and (>= (length pkg-name) (length prefix-upper))
            (string= prefix-upper pkg-name :end2 (length prefix-upper)))))
   all-symbols))

(defun filter-by-symbol-name (prefix-upper all-symbols)
  "Filter symbols by symbol name prefix"
  (let ((search-prefix (if (and (> (length prefix-upper) 0)
                               (char= (char prefix-upper 0) #\:))
                          (subseq prefix-upper 1)
                          prefix-upper)))
    (remove-if-not
     (lambda (sym)
       (let ((sym-name (getf sym :symbol)))
         (and (>= (length sym-name) (length search-prefix))
              (string= search-prefix sym-name :end2 (length search-prefix)))))
     all-symbols)))

(defun filter-symbols-by-prefix (prefix all-symbols)
  "Filter symbols by prefix (package name or symbol name)"
  (let* ((prefix-upper (string-upcase prefix))
         (exact-match (filter-by-exact-package prefix-upper all-symbols)))
    (if exact-match
        exact-match
        (let ((partial-match (filter-by-partial-package prefix-upper all-symbols)))
          (if (not (null partial-match))
              partial-match
              (filter-by-symbol-name prefix-upper all-symbols))))))

(defun symbol-sort-tier (pkg-name)
  "Get sort tier for package (0=KEYWORD, 1=COMMON-LISP, 2=other)"
  (cond ((string= pkg-name "KEYWORD") 0)
        ((string= pkg-name "COMMON-LISP") 1)
        (t 2)))

(defun sort-symbols-by-priority (symbols)
  "Sort symbols by priority: KEYWORD, COMMON-LISP, then others alphabetically"
  (sort symbols
        (lambda (a b)
          (let ((tier-a (symbol-sort-tier (getf a :package)))
                (tier-b (symbol-sort-tier (getf b :package))))
            (if (= tier-a tier-b)
                (string< (getf a :symbol) (getf b :symbol))
                (< tier-a tier-b))))))

(defun handle-list-symbols-message (msg-id prefix stream)
  "Handle list-symbols message type"
  (let* ((all-symbols (collect-user-symbols))
         (symbols (if prefix
                     (filter-symbols-by-prefix prefix all-symbols)
                     all-symbols)))
    (setf symbols (sort-symbols-by-priority symbols))
    (write-message stream (list :id msg-id :symbols symbols))))

(defun handle-symbol-info-message (msg-id symbol-name package-name stream)
  "Handle symbol-info message type"
  (let ((info (get-symbol-info symbol-name package-name)))
    (if info
        (write-message stream (append (list :id msg-id) info))
        (write-message stream (list :id msg-id :error "Symbol not found")))))


(defun dispatch-message (msg-type msg-id message stream)
  "Dispatch message to appropriate handler"
  (cond
    ((string= msg-type "eval")
     (handle-eval-message msg-id
                         (getf message :code)
                         (getf message :file-path)
                         (getf message :file-line)
                         (getf message :file-character)
                         stream))
    ((string= msg-type "ping")
     (handle-ping-message msg-id stream))
    ((string= msg-type "list-symbols")
     (handle-list-symbols-message msg-id (getf message :prefix) stream))
    ((string= msg-type "symbol-info")
     (handle-symbol-info-message msg-id (getf message :symbol)
                                (getf message :package) stream))
    (t (log-error "Unknown message type: ~A" msg-type))))

(defun handle-request (stream message)
  "Handle a single request and send response via stream"
  (log-error "Received message: ~S" message)
  (dispatch-message (getf message :type)
                   (getf message :id)
                   message
                   stream))

;;;; Socket Server Integration

(defun message-handler (message stream)
  "Handle incoming message from socket server"
  (handle-request stream message))

(defun start-master-repl ()
  "Start the master REPL server with Unix socket"
  (log-error "========================================")
  (log-error "Master REPL Server (Unix Socket)")
  (log-error "Shared environment for all kernels")
  (log-error "========================================")
  (zed-cl.socket-server:start-socket-server #'message-handler))

;;;; Entry point
;; This function is exported and called by start-master-repl.lisp entry point
;; No auto-start - use ASDF loading instead
