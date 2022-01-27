use crate::compile::checks::tree_visitor::Visitor;
use crate::compile::checks::{Annotations, VariableType};
use crate::parsing::ast::{Program, Stmt};
use crate::parsing::lexer::{Index, Token, TokenKind};
use crate::Expr;
use std::collections::HashMap;

pub struct AnnotationGenerator<'a> {
    annotations: &'a mut Annotations,

    scopes: Vec<(ScopeType, Token, HashMap<String, bool>)>,
    blocks: Vec<Token>,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ScopeType {
    Block,
    Function,
}

enum LookupResult {
    FoundInitInLocal(Token),
    FoundAnyInOuter(Token, usize),
    NotFound,
}

impl<'a> AnnotationGenerator<'a> {
    pub fn generate_annotations(
        ast: Program,
        annotations: &'a mut Annotations,
    ) -> Result<Program, String> {
        let mut annotator = AnnotationGenerator {
            annotations,
            scopes: Default::default(),
            blocks: Default::default(),
        };

        let block_id = match &ast {
            Program::Block(start, ..) => start,
            _ => &Token {
                kind: TokenKind::BeginBlock,
                position: Index(0, 0),
            },
        };

        annotator.new_scope(ScopeType::Block, block_id);

        annotator.visit_expr(ast)
    }

    fn current_block(&self) -> &Token {
        self.blocks.last().unwrap()
    }

    fn declare_name(&mut self, variable_name: &Token) {
        self.scopes
            .last_mut()
            .unwrap()
            .2
            .insert(variable_name.get_string().unwrap().to_string(), false);

        self.annotations
            .get_or_create_block_scope(&self.scopes.last_mut().unwrap().1)
            .insert(
                variable_name.get_string().unwrap().to_string(),
                VariableType::Normal,
            );
    }

    fn define_name(&mut self, variable_name: &Token) {
        self.scopes
            .last_mut()
            .unwrap()
            .2
            .insert(variable_name.get_string().unwrap().to_string(), true);
    }

    fn lookup_local(&self, variable_name: &str) -> bool {
        //try to lookup initialized value
        for (scope_type, _scope_identifier, scope_map) in self.scopes.iter().rev() {
            if let Some(true) = scope_map.get(variable_name) {
                return true;
            }

            if *scope_type == ScopeType::Function {
                break;
            }
        }
        false
    }

    fn lookup_outer(&self, variable_name: &str) -> bool {
        let mut passed_function_scope = false;
        for (scope_type, _scope_identifier, scope_map) in self.scopes.iter().rev() {
            if passed_function_scope {
                if scope_map.contains_key(variable_name) {
                    return true;
                }
            } else if *scope_type == ScopeType::Function {
                passed_function_scope = true;
            }
        }
        false
    }

    fn lookup_name(&mut self, variable_name: &str) {
        if self.lookup_local(variable_name) {
            return;
        }

        if !self.lookup_outer(variable_name) {
            return;
        }

        let mut passed_function_scope = false;
        for (scope_type, scope_identifier, scope_map) in self.scopes.iter().rev() {
            if !passed_function_scope && *scope_type == ScopeType::Function {
                passed_function_scope = true;
                self.annotations
                    .get_or_create_closure_scope(scope_identifier)
                    .insert(variable_name.to_string());
            } else {
                if scope_map.contains_key(variable_name) {
                    self.annotations
                        .get_or_create_block_scope(scope_identifier)
                        .insert(variable_name.to_string(), VariableType::Boxed);
                    return;
                }
                if *scope_type == ScopeType::Function {
                    self.annotations
                        .get_or_create_closure_scope(scope_identifier)
                        .insert(variable_name.to_string());
                }
            }
        }
    }

    fn new_scope(&mut self, scope_type: ScopeType, token: &Token) {
        self.scopes
            .push((scope_type, token.clone(), Default::default()));
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

impl<'a> Visitor<String> for AnnotationGenerator<'a> {
    fn visit_var_stmt(&mut self, name: Token, mut rhs: Option<Expr>) -> Result<Stmt, String> {
        if let Some(value) = rhs {
            rhs = Some(self.visit_expr(value)?);
        }

        self.define_name(&name);

        Ok(Stmt::VarDeclaration(name, rhs))
    }

    fn visit_function_declaration_statement(
        &mut self,
        name: Token,
        args: Vec<Token>,
        body: Expr,
    ) -> Result<Stmt, String> {
        self.new_scope(ScopeType::Function, &name);
        self.annotations.get_or_create_closure_scope(&name);
        for arg_name in &args {
            self.declare_name(arg_name);
            self.define_name(arg_name);
        }
        self.define_name(&name);
        let body = self.visit_expr(body)?;
        self.pop_scope();

        Ok(Stmt::FunctionDeclaration { name, args, body })
    }

    fn visit_variable_expr(&mut self, variable_name: Token) -> Result<Expr, String> {
        self.lookup_name(variable_name.get_string().unwrap());
        Ok(Expr::Name(variable_name))
    }

    fn visit_block(
        &mut self,
        start_token: Token,
        end_token: Token,
        containing_statements: Vec<Stmt>,
    ) -> Result<Expr, String> {
        self.new_scope(ScopeType::Block, &start_token);
        self.annotations.get_or_create_block_scope(&start_token);

        //declare variables
        for statement in &containing_statements {
            match statement {
                Stmt::VarDeclaration(name, _) => {
                    self.declare_name(name);
                }
                Stmt::FunctionDeclaration { name, .. } => {
                    self.declare_name(name);
                }
                _ => {}
            }
        }

        let mut statements = vec![];
        for item in containing_statements {
            statements.push(self.visit_stmt(item)?);
        }

        self.pop_scope();
        Ok(Expr::Block(start_token, end_token, statements))
    }

    fn visit_anon_function_expr(
        &mut self,
        args: Vec<Token>,
        arrow: Token,
        body: Box<Expr>,
    ) -> Result<Expr, String> {
        self.new_scope(ScopeType::Function, &arrow);
        self.annotations.get_or_create_closure_scope(&arrow);
        for arg_name in &args {
            self.declare_name(arg_name);
            self.define_name(arg_name);
        }

        let body = self.visit_expr(*body)?;
        self.pop_scope();
        Ok(Expr::AnonFunction(args, arrow, Box::new(body)))
    }
}