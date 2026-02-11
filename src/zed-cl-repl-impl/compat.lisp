;;;; compat.lisp - Multi-implementation compatibility layer
;;;;
;;;; This layer abstracts implementation-specific APIs to support multiple
;;;; Common Lisp implementations (SBCL, ECL, etc.)

;; Load ASDF/UIOP for portable environment functions
(require :asdf)

(defpackage :zed-cl.compat
  (:use :cl)
  (:export
   ;; Introspection
   #:get-lambda-list
   #:get-function-type
   #:get-source-location

   ;; Error handling
   #:get-backtrace

   ;; Environment
   #:quit
   #:getenv
   #:setenv

   ;; Package classification
   #:system-package-p

   ;; Compilation
   #:with-source-tracking))

(in-package :zed-cl.compat)

;;;; Lambda List Introspection

#+sbcl
(eval-when (:compile-toplevel :load-toplevel :execute)
  (require :sb-introspect))

#+sbcl
(defun get-lambda-list (symbol)
  "Get function lambda list (SBCL)"
  (sb-introspect:function-lambda-list symbol))

#+ecl
(defun get-lambda-list (symbol)
  "Get function lambda list (ECL)"
  (ext:function-lambda-list symbol))

#-(or sbcl ecl)
(defun get-lambda-list (symbol)
  "Get function lambda list (fallback)"
  (declare (ignore symbol))
  nil)

;;;; Function Type Information

#+sbcl
(defun get-function-type (symbol)
  "Get function type information (SBCL)"
  (sb-introspect:function-type symbol))

#+ecl
(defun get-function-type (symbol)
  "Get function type information (ECL - not available)"
  (declare (ignore symbol))
  nil)

#-(or sbcl ecl)
(defun get-function-type (symbol)
  "Get function type information (fallback)"
  (declare (ignore symbol))
  nil)

;;;; Source Location

#+sbcl
(defun get-source-location (symbol)
  "Get source location for symbol (SBCL)"
  (handler-case
      (when (fboundp symbol)
        (let* ((def-source (sb-introspect:find-definition-source
                            (symbol-function symbol)))
               (pathname (sb-introspect:definition-source-pathname def-source))
               (char-offset (sb-introspect:definition-source-character-offset def-source)))
          (when pathname
            ;; Translate logical pathnames to physical paths
            (let* ((physical-path (if (typep pathname 'logical-pathname)
                                      (handler-case
                                          (translate-logical-pathname pathname)
                                        (error () pathname))
                                      pathname))
                   (path-string (namestring physical-path)))
              (list path-string nil char-offset)))))
    (error () nil)))

#+ecl
(defun get-source-location (symbol)
  "Get source location for symbol (ECL - runtime introspection not available)"
  (declare (ignore symbol))
  ;; ECL's si:compiled-function-file returns NIL
  ;; Source location will come from indexer instead
  nil)

#-(or sbcl ecl)
(defun get-source-location (symbol)
  "Get source location for symbol (fallback)"
  (declare (ignore symbol))
  nil)

;;;; Backtrace Support

#+sbcl
(defun get-backtrace ()
  "Get current backtrace (SBCL)"
  (with-output-to-string (s)
    ;; sb-debug is part of SBCL core, no require needed
    (let ((frames (or (ignore-errors (sb-debug:list-backtrace)) '())))
      (loop for frame in (subseq frames 0 (min 10 (length frames)))
            do (format s "~A~%" frame)))))

#+ecl
(defun get-backtrace ()
  "Get current backtrace (ECL - limited)"
  ;; ECL doesn't have good Lisp-level backtrace
  ;; Return simple message
  "Backtrace not available on ECL")

#-(or sbcl ecl)
(defun get-backtrace ()
  "Get current backtrace (fallback)"
  "Backtrace not available")

;;;; Environment Variables

(defun getenv (name)
  "Get environment variable (portable via UIOP)"
  (uiop:getenv name))

(defun setenv (name value)
  "Set environment variable (portable via UIOP)"
  (setf (uiop:getenv name) value))

(defun quit (&optional (code 0))
  "Exit the Lisp process (portable via UIOP)"
  (uiop:quit code))

;;;; Package Classification

(defun implementation-system-packages ()
  "Get list of implementation-specific system package names"
  #+sbcl
  '()  ; SBCL system packages all start with SB-
  #+ecl
  '("EXT" "SI" "GRAY" "C" "FFI" "CLOS" "MP" "WALKER" "ECL-CDB")
  #-(or sbcl ecl)
  '())

(defun system-package-prefix-p (pkg-name prefix)
  "Check if package name starts with prefix"
  (and (>= (length pkg-name) (length prefix))
       (string= prefix pkg-name :end2 (length prefix))))

(defun system-package-p (pkg-name)
  "Check if package is a system package (implementation-aware)"
  (or
   ;; Common system packages
   (string= pkg-name "COMMON-LISP")
   (string= pkg-name "KEYWORD")

   ;; ASDF and subsystems
   (or (string= pkg-name "ASDF")
       (system-package-prefix-p pkg-name "ASDF/"))

   ;; Quicklisp and subsystems
   (or (string= pkg-name "QUICKLISP-CLIENT")
       (system-package-prefix-p pkg-name "QL-")
       (system-package-prefix-p pkg-name "QL/"))

   ;; UIOP and subsystems
   (or (string= pkg-name "UIOP")
       (system-package-prefix-p pkg-name "UIOP/"))

   ;; SBCL-specific
   #+sbcl (system-package-prefix-p pkg-name "SB-")

   ;; Implementation-specific packages
   (member pkg-name (implementation-system-packages) :test #'string=)))

;;;; Compilation with Source Tracking

#+sbcl
(defmacro with-source-tracking ((file-path) &body body)
  "Track source file during compilation (SBCL)"
  `(let ((sb-c::*source-namestring* ,file-path))
     ,@body))

#+ecl
(defmacro with-source-tracking ((file-path) &body body)
  "Track source file during compilation (ECL - not supported)"
  (declare (ignore file-path))
  `(progn ,@body))

#-(or sbcl ecl)
(defmacro with-source-tracking ((file-path) &body body)
  "Track source file during compilation (fallback)"
  (declare (ignore file-path))
  `(progn ,@body))
