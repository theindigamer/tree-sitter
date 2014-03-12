#include "compiler/prepare_grammar/extract_tokens.h"
#include <map>
#include <string>
#include "tree_sitter/compiler.h"
#include "compiler/prepared_grammar.h"
#include "compiler/rules/visitor.h"
#include "compiler/rules/seq.h"
#include "compiler/rules/choice.h"
#include "compiler/rules/repeat.h"
#include "compiler/rules/blank.h"
#include "compiler/rules/symbol.h"
#include "compiler/rules/string.h"
#include "compiler/rules/pattern.h"

namespace tree_sitter {
    using std::pair;
    using std::string;
    using std::to_string;
    using std::map;
    using std::make_shared;
    using rules::rule_ptr;

    namespace prepare_grammar {
        class IsToken : public rules::RuleFn<bool> {
            void default_visit(const rules::Rule *rule) {
                value = false;
            }

            void visit(const rules::String *rule) {
                value = true;
            }

            void visit(const rules::Pattern *rule) {
                value = true;
            }
        };

        class TokenExtractor : public rules::RuleFn<rule_ptr> {
            string add_token(const rule_ptr &rule) {
                for (auto pair : tokens)
                    if (*pair.second == *rule)
                        return pair.first;
                string name = "token" + to_string(tokens.size() + 1);
                tokens.insert({ name, rule });
                return name;
            }

            void default_visit(const rules::Rule *rule) {
                auto result = rule->copy();
                if (IsToken().apply(result)) {
                    value = make_shared<rules::Symbol>(add_token(result), rules::SymbolTypeAuxiliary);
                } else {
                    value = result;
                }
            }

            void visit(const rules::Choice *rule) {
                value = rules::Choice::Build({ apply(rule->left), apply(rule->right) });
            }

            void visit(const rules::Seq *rule) {
                value = rules::Seq::Build({ apply(rule->left), apply(rule->right) });
            }

            void visit(const rules::Repeat *rule) {
                value = make_shared<rules::Repeat>(apply(rule->content));
            }

        public:
            map<const string, const rule_ptr> tokens;
        };

        pair<PreparedGrammar, PreparedGrammar> extract_tokens(const PreparedGrammar &input_grammar) {
            map<const string, const rule_ptr> rules, tokens, aux_rules, aux_tokens;
            TokenExtractor extractor;

            for (auto &pair : input_grammar.rules) {
                string name = pair.first;
                rule_ptr rule = pair.second;
                if (IsToken().apply(rule))
                    tokens.insert({ name, rule });
                else
                    rules.insert({ name, extractor.apply(rule) });
            }

            for (auto &pair : input_grammar.aux_rules) {
                string name = pair.first;
                rule_ptr rule = pair.second;
                if (IsToken().apply(rule))
                    aux_tokens.insert({ name, rule });
                else
                    aux_rules.insert({ name, extractor.apply(rule) });
            }

            aux_tokens.insert(extractor.tokens.begin(), extractor.tokens.end());

            return {
                PreparedGrammar(input_grammar.start_rule_name, rules, aux_rules),
                PreparedGrammar("", tokens, aux_tokens)
            };
        }
    }
}