;;;; config.lisp - Configuration parameters for zed-cl
;;;;
;;;; Centralized configuration for all zed-cl components

(defpackage :zed-cl.config
  (:use :cl)
  (:export
   ;; Completion settings
   #:*completion-package-whitelist*
   ;; Profile-based config
   #:get-active-profile
   #:get-profile-setting))

(in-package :zed-cl.config)

;;;; Configuration File Path

(defparameter *config-path*
  (merge-pathnames ".zed-cl/config.json" (user-homedir-pathname))
  "Path to configuration file")

;;;; Profile-based Configuration

(defvar *cached-config* nil
  "Cached configuration to avoid re-reading file")

(defun read-config ()
  "Read and parse ~/.zed-cl/config.json"
  (when (probe-file *config-path*)
    (handler-case
        (with-open-file (stream *config-path* :direction :input)
          (cl-json:decode-json stream))
      (error (e)
        (format *error-output* "~&[Config] Error reading config: ~A~%" e)
        nil))))

(defun get-config ()
  "Get cached config or read it"
  (unless *cached-config*
    (setf *cached-config* (read-config)))
  *cached-config*)

(defun get-active-profile ()
  "Get the name of the active profile (default: \"sbcl\")"
  (let ((config (get-config)))
    (or (cdr (assoc :active--profile config))
        "sbcl")))

(defun get-default-profile ()
  "Get the default profile settings"
  (list (cons :lisp--impl "sbcl")
        (cons :system--index "system-index.db")
        (cons :completion--package--whitelist nil)))

(defun get-profile-setting (setting-name &optional default)
  "Get a setting from the active profile"
  (let* ((config (get-config))
         (active-profile-name (get-active-profile))
         (profiles (cdr (assoc :profiles config)))
         (profile-key (intern (string-upcase active-profile-name) :keyword))
         (active-profile (cdr (assoc profile-key profiles))))
    ;; If no profile found, use defaults
    (unless active-profile
      (setf active-profile (get-default-profile)))
    (or (cdr (assoc setting-name active-profile))
        default)))

;;;; Completion Configuration

(defparameter *completion-package-whitelist* :unset
  "List of package names to include in completions.
   If :unset (default), includes all user-defined packages.
   If set to a list, ONLY includes packages explicitly listed (no magic defaults).")

;; Initialize whitelist from profile config
(let ((whitelist (get-profile-setting :completion--package--whitelist)))
  (when whitelist
    (setf *completion-package-whitelist* (mapcar #'string-upcase whitelist))))
