from conan import ConanFile
from conan.tools.build import check_min_cppstd
from conan.tools.cmake import CMakeToolchain, CMake, cmake_layout
import os

class CryFSConan(ConanFile):
    name = "cryfs"
    version = "1.0"
    settings = "os", "compiler", "build_type", "arch"
    tool_requires = "cmake/3.25.3"
    generators = ["CMakeToolchain", "CMakeDeps"]
    package_folder = "/usr"
    options = {
        "build_tests": [True, False],
        "update_checks": [True, False],
        "disable_openmp": [True, False],

        # The following options are helpful for development and/or CI
        "use_werror": [True, False],
        "use_clang_tidy": [True, False],
        "export_compile_commands": [True, False],
        "use_iwyu": [True, False],
        "clang_tidy_warnings_as_errors": [True, False],
        "windows_dokany_path": ["ANY"],
        "use_ccache": [True, False],
    }
    default_options = {
        "build_tests": False,
        "update_checks": True,
        "disable_openmp": False,
        "use_werror": False,
        "use_clang_tidy": False,
        "export_compile_commands": False,
        "use_iwyu": False,
        "clang_tidy_warnings_as_errors": False,
        "windows_dokany_path": "",
        "use_ccache": "False",
        # Options of our dependencies
        "boost/*:system_no_deprecated": True,
        "boost/*:asio_no_deprecated": True,
        "boost/*:filesystem_no_deprecated": True,
        "boost/*:without_atomic": False,  # needed by boost thread
        "boost/*:without_chrono": False,  # needed by CryFS
        "boost/*:without_cobalt": True,
        "boost/*:without_container": False,  # needed by boost thread
        "boost/*:without_context": True,
        "boost/*:without_contract": True,
        "boost/*:without_coroutine": True,
        "boost/*:without_date_time": False,  # needed by boost thread
        "boost/*:without_exception": False,  # needed by boost thread
        "boost/*:without_fiber": True,
        "boost/*:without_filesystem": False,  # needed by CryFS
        "boost/*:without_graph": True,
        "boost/*:without_graph_parallel": True,
        "boost/*:without_iostreams": True,
        "boost/*:without_json": True,
        "boost/*:without_locale": True,
        "boost/*:without_log": True,
        "boost/*:without_math": True,
        "boost/*:without_mpi": True,
        "boost/*:without_nowide": True,
        "boost/*:without_program_options": False,  # needed by CryFS
        "boost/*:without_python": True,
        "boost/*:without_random": True,
        "boost/*:without_regex": True,
        "boost/*:without_serialization": False,  # needed by boost date_time
        # Stacktrace is needed by CryFS. Stacktrace is a header-only library and linking against its static version actually **disables** stacktraces,
        # see https://www.boost.org/doc/libs/1_65_0/doc/html/stacktrace/getting_started.html#stacktrace.getting_started.enabling_and_disabling_stacktrac
        # This is why we need to **not** link against the static version of stacktrace.
        "boost/*:without_stacktrace": True,
        "boost/*:without_system": False,  # needed by CryFS
        "boost/*:without_test": True,
        "boost/*:without_thread": False,  # needed by CryFS
        "boost/*:without_timer": True,
        "boost/*:without_type_erasure": True,
        "boost/*:without_url": True,
        "boost/*:without_wave": True,
    }

    def validate(self):
        check_min_cppstd(self, "17")
     
    def requirements(self):
        self.requires("range-v3/0.12.0")
        self.requires("spdlog/1.14.1")
        self.requires("boost/1.84.0")
        if self.options.update_checks:
            self.requires("libcurl/8.9.1")
        if self.options.build_tests:
            self.requires("gtest/1.15.0")

    def layout(self):
        cmake_layout(self)

    def build(self):
        cmake = CMake(self)
        cmake_vars = {
            "BUILD_TESTING": self.options.build_tests,
            "CRYFS_UPDATE_CHECKS": self.options.update_checks,
            "DISABLE_OPENMP": self.options.disable_openmp,
            "USE_WERROR": self.options.use_werror,
            "USE_CLANG_TIDY": self.options.use_clang_tidy,
            "CMAKE_EXPORT_COMPILE_COMMANDS": self.options.export_compile_commands,
            "USE_IWYU": self.options.use_iwyu,
            "CLANG_TIDY_WARNINGS_AS_ERRORS": self.options.clang_tidy_warnings_as_errors,
        }
        if self.options.use_ccache:
            cmake_vars["CMAKE_C_COMPILER_LAUNCHER"] = "ccache"
            cmake_vars["CMAKE_CXX_COMPILER_LAUNCHER"] = "ccache"
            # ccache is incomptible with `/Zi` or `/ZI` and needs `/Z7`, see
            # - https://discourse.cmake.org/t/early-experiences-with-msvc-debug-information-format-and-cmp0141/6859
            # - https://learn.microsoft.com/en-us/cpp/build/reference/z7-zi-zi-debug-information-format?view=msvc-170
            # - https://cmake.org/cmake/help/latest/variable/CMAKE_MSVC_DEBUG_INFORMATION_FORMAT.html
            cmake_vars["CMAKE_MSVC_DEBUG_INFORMATION_FORMAT"] = "Embedded"
        if self.options.windows_dokany_path != "":
            cmake_vars["DOKAN_PATH"] = self.options.windows_dokany_path
        cmake.configure(cmake_vars)
        cmake.build()
