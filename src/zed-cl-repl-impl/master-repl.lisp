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
  (:import-from :zed-cl.socket-server
                #:write-frame
                #:client-interrupt)
  (:export #:start-master-repl))

(in-package :zed-cl.master-repl)

;;;; Global state

(defparameter *master-running* t
  "Is master REPL running?")

(defvar *current-file* nil)

(defvar *request-lock*
  #+sbcl (sb-thread:make-mutex :name "zed-cl-request")
  #+ecl (mp:make-lock)
  #-(or sbcl ecl) nil)

(defvar *eval-thread* nil
  "Thread currently running an eval, so an interrupt request from another
connection can target it. NIL when no eval is in flight.")

(defparameter *max-completion-symbols* 500)

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
  (handler-case
      (zed-cl.socket-server:write-frame stream message)
    (error (e) (log-error "Error writing message: ~A" e) nil)))

;;;; Symbol Classification

(defun user-package-p (pkg)
  "Check if package is a user package (not a system package)"
  (when pkg
    (let ((pkg-name (package-name pkg)))
      (not (or (zed-cl.compat:system-package-p pkg-name)
               (string= pkg-name "ZED-CL"))))))

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
  (let ((location (zed-cl.compat:get-source-location sym)))
    (when location
      (let ((path (remap-sbcl-source-path (first location)))
            (line (second location))
            (char (third location)))
        (when path
          (list path
                (and (integerp line) (plusp line) line)
                (and (integerp char) (plusp char) char)))))))

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

(defun prefix-matches (name prefix)
  (or (zerop (length prefix))
      (and (>= (length name) (length prefix))
           (string= prefix name :end2 (length prefix)))))

(defun parse-completion-query (prefix package-name)
  (let* ((raw (string-upcase (or prefix "")))
         (pkg-arg (and package-name (string-upcase (string package-name)))))
    (cond
      ((and pkg-arg (plusp (length pkg-arg)))
       (values pkg-arg
               (if (and (plusp (length raw)) (char= (char raw 0) #\:))
                   (subseq raw 1)
                   raw)))
      (t
       (let ((double (search "::" raw)))
         (cond
           (double
            (values (subseq raw 0 double) (subseq raw (+ double 2))))
           ((and (plusp (length raw)) (char= (char raw 0) #\:))
            (values "KEYWORD" (subseq raw 1)))
           (t
            (let ((colon (position #\: raw)))
              (if colon
                  (values (subseq raw 0 colon) (subseq raw (1+ colon)))
                  (values nil raw))))))))))

(defun collect-symbol-info-light (sym pkg kind)
  (when kind
    (list :symbol (symbol-name sym)
          :kind kind
          :package (package-name pkg))))

(defun collect-matching-symbols (prefix &optional package-name)
  (multiple-value-bind (pkg-name prefix-upper)
      (parse-completion-query prefix package-name)
    (let ((symbols nil)
          (count 0)
          (seen (make-hash-table :test 'equal)))
      (labels ((add-from-package (pkg)
                 (let ((this-name (package-name pkg)))
                   (do-symbols (sym pkg)
                     (when (>= count *max-completion-symbols*)
                       (return))
                     (when (and (eq (symbol-package sym) pkg)
                                (prefix-matches (symbol-name sym) prefix-upper))
                       (let ((key (cons this-name (symbol-name sym)))
                             (kind (symbol-kind
                                    sym
                                    (string= this-name "COMMON-LISP")
                                    (string= this-name "KEYWORD"))))
                         (when (and kind
                                    (not (gethash key seen))
                                    (or (not (string= this-name "KEYWORD"))
                                        (whitelisted-keyword-p sym)))
                           (setf (gethash key seen) t)
                           (let ((info (collect-symbol-info-light sym pkg kind)))
                             (when info
                               (push info symbols)
                               (incf count))))))))))
        (if pkg-name
            (let ((pkg (find-package pkg-name)))
              (when pkg
                (add-from-package pkg)))
            (dolist (pkg (list-all-packages))
              (when (and (< count *max-completion-symbols*)
                         (whitelisted-package-p pkg))
                (let ((this-name (package-name pkg)))
                  (when (and (plusp (length prefix-upper))
                             (prefix-matches this-name prefix-upper))
                    (let ((info (get-package-info this-name)))
                      (when info
                        (push info symbols)
                        (incf count))))
                  (add-from-package pkg))))))
      symbols)))

;;;; Symbol Information

(defun find-symbol-in-user-packages (symbol-name)
  "Find symbol in user-defined packages"
  (block found
    (do-all-symbols (s)
      (when (and (string= (symbol-name s) (string-upcase symbol-name))
                 (symbol-package s)
                 (user-package-p (symbol-package s)))
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
                (append (list :source-file (first source-loc))
                        (when (second source-loc)
                          (list :source-line (second source-loc)))
                        (when (third source-loc)
                          (list :source-character (third source-loc)))))))))

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

(defun eval-one-form (form &optional file-path)
  (if file-path
      (with-source-tracking (file-path)
        (multiple-value-list (eval form)))
      (multiple-value-list (eval form))))

(defun eval-forms-from-code (code &optional file-path)
  (when file-path
    (setf *current-file* file-path))
  (let ((values nil))
    (with-input-from-string (stream code)
      (loop for form = (read stream nil :eof)
            until (eq form :eof)
            do (setf values (eval-one-form form file-path))))
    values))

(defun capture-backtrace ()
  "Capture current backtrace as string"
  (get-backtrace))

(defun eval-with-output-capture (code &optional file-path)
  "Evaluate code with output capture, return (values error displays)"
  (let ((output (make-string-output-stream))
        (backtrace nil))
    (handler-case
        ;; HANDLER-CASE unwinds the stack before running a clause, so the
        ;; backtrace must be captured here, at signal time, while the frames
        ;; below the error still exist.
        (handler-bind ((error (lambda (e)
                                (declare (ignore e))
                                (setf backtrace (capture-backtrace)))))
          (let ((*standard-output* output)
                (*error-output* output)
                (*trace-output* output)
                #+sbcl (sb-ext:*muffled-warnings* nil))
            (clear-display-outputs)
            (let ((values (eval-forms-from-code code file-path)))
              (update-package-whitelist)
              (list values (get-output-stream-string output) nil nil
                    (collect-display-outputs)))))
      (end-of-file ()
        (list nil (get-output-stream-string output)
              "Incomplete code: unbalanced parentheses"
              (list "Incomplete code") nil))
      (error (e)
        (list nil (get-output-stream-string output)
              (format nil "~A" e)
              (or backtrace (capture-backtrace)) nil)))))

(defun eval-code (code &optional file-path)
  "Evaluate code in the master REPL, return (output values error traceback displays)"
  (destructuring-bind (values output error traceback displays)
      (eval-with-output-capture code file-path)
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
                        :values (mapcar #'prin1-to-string (getf result :values))
                        :error (getf result :error)
                        :traceback (getf result :traceback))))
    (when (getf result :displays)
      (setf response (append response (list :displays (getf result :displays)))))
    response))

(defun current-thread ()
  #+sbcl sb-thread:*current-thread*
  #+ecl mp:*current-process*
  #-(or sbcl ecl) nil)

(defmacro with-deferred-interrupts (&body body)
  "Delay thread interruptions so a response frame is written whole."
  #+sbcl `(sb-sys:without-interrupts ,@body)
  #+ecl `(mp:without-interrupts ,@body)
  #-(or sbcl ecl) `(progn ,@body))

(defun handle-eval-message (msg-id code file-path stream)
  "Handle eval message type"
  (log-error "Eval request: ~A from ~A"
             (subseq code 0 (min 50 (length code)))
             (or file-path "interactive"))
  (setf *eval-thread* (current-thread))
  (let ((result (unwind-protect
                     (eval-code code file-path)
                  (setf *eval-thread* nil)))
        (sent nil))
    ;; An interrupt aimed at the eval can land after it finished. The client
    ;; has no read deadline on evals, so the response must go out exactly
    ;; once even if the interruption unwinds this frame mid-flight.
    (flet ((send-once ()
             (with-deferred-interrupts
               (unless sent
                 (setf sent t)
                 (write-message stream (build-eval-response msg-id result))))))
      (handler-case (send-once)
        (client-interrupt () (send-once))))))

(defun handle-interrupt-message (msg-id stream)
  "Abort the eval running in another connection's thread, if any."
  (let ((thread *eval-thread*))
    (if thread
        (progn
          (log-error "Interrupting eval thread")
          #+sbcl (ignore-errors
                   (sb-thread:interrupt-thread
                    thread (lambda () (error 'client-interrupt))))
          #+ecl (ignore-errors
                  (mp:interrupt-process
                   thread (lambda () (error 'client-interrupt))))
          #-(or sbcl ecl) nil)
        (log-error "Interrupt requested but no eval running")))
  (write-message stream (list :id msg-id :ok t)))

(defun handle-ping-message (msg-id stream)
  "Handle ping message type"
  (write-message stream (list :id msg-id :pong t)))

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

(defun handle-list-symbols-message (msg-id prefix package-name stream)
  (let ((symbols (sort-symbols-by-priority
                  (collect-matching-symbols prefix package-name))))
    (write-message stream (list :id msg-id :symbols symbols))))

(defun handle-symbol-info-message (msg-id symbol-name package-name stream)
  "Handle symbol-info message type"
  (let ((info (get-symbol-info symbol-name package-name)))
    (if info
        (write-message stream (append (list :id msg-id) info))
        (write-message stream (list :id msg-id :error "Symbol not found")))))

(defun handle-set-current-file (msg-id path contents stream)
  (declare (ignore contents))
  (when path
    (setf *current-file* path)
    (log-error "Current file: ~A" path))
  (write-message stream (list :id msg-id :ok t)))

(defun dispatch-message (msg-type msg-id message stream)
  "Dispatch message to appropriate handler"
  (cond
    ((string= msg-type "eval")
     (handle-eval-message msg-id
                          (getf message :code)
                          (getf message :file-path)
                          stream))
    ((string= msg-type "ping")
     (handle-ping-message msg-id stream))
    ((string= msg-type "set-current-file")
     (handle-set-current-file msg-id (getf message :path)
                              (getf message :contents) stream))
    ((string= msg-type "list-symbols")
     (handle-list-symbols-message msg-id (getf message :prefix)
                                  (getf message :package) stream))
    ((string= msg-type "symbol-info")
     (handle-symbol-info-message msg-id (getf message :symbol)
                                 (getf message :package) stream))
    ((string= msg-type "interrupt")
     (handle-interrupt-message msg-id stream))
    (t (log-error "Unknown message type: ~A" msg-type)
       ;; Always answer, or the client blocks until its read times out.
       (write-message stream
                      (list :id (or msg-id "unknown")
                            :error (format nil "Unknown message type: ~A" msg-type))))))

(defun handle-request (stream message)
  (log-error "~A id=~A" (getf message :type) (getf message :id))
  (dispatch-message (getf message :type)
                    (getf message :id)
                    message
                    stream))

;;;; Socket Server Integration

(defun lock-free-request-p (message)
  "Interrupt (and ping) must not queue behind the eval they are meant to
reach, so they bypass the request mutex. Both only write to their own
connection's stream."
  (member (getf message :type) '("interrupt" "ping") :test #'equal))

(defun message-handler (message stream)
  (if (lock-free-request-p message)
      (handle-request stream message)
      (progn
        #+sbcl
        (sb-thread:with-mutex (*request-lock*)
          (handle-request stream message))
        #+ecl
        (mp:with-lock (*request-lock*)
          (handle-request stream message))
        #-(or sbcl ecl)
        (handle-request stream message))))

(defun start-master-repl ()
  "Start the master REPL TCP server on 127.0.0.1"
  (log-error "========================================")
  (log-error "Master REPL Server (TCP 127.0.0.1)")
  (log-error "Shared environment for all kernels")
  (log-error "========================================")
  (zed-cl.socket-server:start-socket-server #'message-handler))

;;;; Entry point
;; This function is exported and called by start-master-repl.lisp entry point
;; No auto-start - use ASDF loading instead
