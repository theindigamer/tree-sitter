const TreeSitter = {};

TreeSitter.init = async (wasmURL) => {
  let heap32 = null;

  const memory = new WebAssembly.Memory({ initial: 256 });
  const table = new WebAssembly.Table({initial: 48, maximum: 48, element: 'anyfunc'});
  const response = await fetch(wasmURL);
  const buffer = await response.arrayBuffer();
  const module = await WebAssembly.instantiate(buffer, {
    env: {
      memory,
      table,
      __table_base: 0,
      DYNAMICTOP_PTR: 0,
      ___assert_fail: (message) => {
        debugger
        throw new Error(message);
      },
      _exit: () => {},
      abort: () => {},
      jsCall_i: () => {},
      jsCall_ii: () => {},
      jsCall_iii: () => {},
      jsCall_iiii: () => {},
      jsCall_iiiii: () => {},
      jsCall_vi: () => {},
      jsCall_vii: () => {},
      jsCall_viii: () => {},
      ___setErrNo: () => {},
      _emscripten_resize_heap: () => {
        memory.grow(1);
        heap32 = null;
      },
      _emscripten_get_heap_size: () => memory.buffer.byteLength,
      _emscripten_memcpy_big: (...args) => {
        console.log('memcpy', memory, args, memory.buffer.length);
      },
      abortOnCannotGrowMemory: () => {},
    },
  });

  function read32(address, offset = 0) {
    if (!heap32) heap32 = new Uint32Array(memory.buffer);
    return heap32[address / heap32.BYTES_PER_ELEMENT + offset];
  }

  function write32(value, address, offset = 0) {
    if (!heap32) heap32 = new Uint32Array(memory.buffer);
    return heap32[address / heap32.BYTES_PER_ELEMENT];
  }

  function writeString(values, address, offset = 0) {
    // if (!heap32) heap32 = new Uint32Array(memory.buffer);
    return memory.buffer.set(values, address / heap32.BYTES_PER_ELEMENT);
  }

  const C = module.instance.exports;

  class Parser {
    constructor() {
      const transferBufferAddress = C._ts_parser_new_wasm();
      this[0] = read32(transferBufferAddress, 0);
      this[1] = read32(transferBufferAddress, 1);
    }

    parse(oldTree, callback) {
      if (typeof callback === 'string') {
        return this.parse((index) => string.slice(index))
      }

      const wrappedCallback = function (byte, row, column) {
        const string = callback(byte / 2, row, column / 2);
        writeString(string, this[1] + read32.BYTES_PER_ELEMENT);
      };

      table.set(1, wrappedCallback);

      const treeAddress = C._ts_parser_parse_wasm(
        this[0],
        this[1],
        oldTree ? oldTree[0] : 0,
        1
      );

      if (!treeAddress) {
        throw new Error('Parsing failed');
      }
      return new Tree(treeAddress);
    }

    setLanguage(language) {
      if (language.constructor !== Language) {
        throw new Error('Argument must be a Language');
      }
      C._ts_parser_set_language(this[0], language[0]);
      if (C._ts_parser_language(this[0]) !== language[0]) {
        throw new Error('Incompatible language');
      }
    }

    setLogger(logger) {
      this.logger = logger;
    }

    delete() {
      C._ts_parser_delete(this[0]);
    }
  }

  class Tree {
    constructor(address) {
      this[0] = address;
    }

    get rootNode() {
    }
  }

  class Node {
    constructor() {
    }
  }

  class Language {
    constructor(address) {
      this[0] = address;
    }

    static async load(url) {
      const response = await fetch(url);
      const buffer = await response.arrayBuffer();
      const module = await WebAssembly.instantiate(buffer, {
        env: {
          memory,
          table: new WebAssembly.Table({
            initial: 8,
            maximum: 8,
            element: 'anyfunc'
          }),
          abort: () => {},
          _iswalpha: (a) => {},
          _iswspace: (a) => {},
          __memory_base: 0,
          __table_base: 0,
        }
      });
      const functionName = Object.keys(module.instance.exports).find(key =>
        key.includes("tree_sitter_")
      );
      const languageAddress = module.instance.exports[functionName]();
      return new Language(languageAddress);
    }
  }

  TreeSitter.Parser = Parser;
  TreeSitter.Tree = Tree;
  TreeSitter.Node = Node;
  TreeSitter.Language = Language;

  return TreeSitter;
};

// ----------------------------

async function init() {
  await TreeSitter.init('/assets/tree-sitter.wasm');

  window.TreeSitter = TreeSitter;
  window.TreeSitterJavaScript = await TreeSitter.Language.load('/assets/tree-sitter-javascript.wasm');

  const p = new TreeSitter.Parser();
  p.setLanguage(TreeSitterJavaScript);
  p.parse("{};");
}

init();
