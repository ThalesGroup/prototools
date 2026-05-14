/*
 * SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
 *
 * SPDX-License-Identifier: MIT
 */

#include <Python.h>

typedef struct TSLanguage TSLanguage;

extern const TSLanguage *tree_sitter_textproto(void);

static PyObject *
_binding_language(PyObject *Py_UNUSED(self), PyObject *Py_UNUSED(args))
{
    return PyCapsule_New(
        (void *)tree_sitter_textproto(),
        "tree_sitter.Language",
        NULL
    );
}

static PyMethodDef methods[] = {
    {"language", _binding_language, METH_NOARGS,
     "Get the tree-sitter language for the textproto grammar."},
    {NULL, NULL, 0, NULL}
};

static struct PyModuleDef module = {
    .m_base    = PyModuleDef_HEAD_INIT,
    .m_name    = "textproto",
    .m_doc     = NULL,
    .m_size    = 0,
    .m_methods = methods,
};

PyMODINIT_FUNC
PyInit_textproto(void)
{
    return PyModule_Create(&module);
}
