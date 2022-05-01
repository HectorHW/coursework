use super::type_builder::TypeBuilder;
use super::types::Type;
use crate::compile::checks::tree_visitor::Visitor;
use crate::compile::checks::Annotations;
use crate::execution::arity::Arity;
use crate::parsing::ast::{Expr, Program, Stmt, TypeMention, TypedName};
use crate::parsing::lexer::{Index, Token, TokenKind};
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug)]
pub enum SomewhereTypeError {
    TypeMismatch { expected: Type, got: Type },
    UnspecifiedBinary { left: Type, op: Token, right: Type },
    UnknownType { value: TypeMention },

    ArityMismatch { expected: Arity, got: usize },

    OperationUnsupported { target: Type, message: String },

    AttributeError { target_type: Type, field: String },
}

#[derive(Debug)]
pub struct LocalizedError {
    error: SomewhereTypeError,
    position: Index,
}

impl From<LocalizedError> for TypeError {
    fn from(e: LocalizedError) -> Self {
        TypeError::LocalizedError(e)
    }
}

impl From<SomewhereTypeError> for TypeError {
    fn from(e: SomewhereTypeError) -> Self {
        TypeError::Somewhere(e)
    }
}

impl SomewhereTypeError {
    pub fn at(self, position: Index) -> LocalizedError {
        LocalizedError {
            error: self,
            position,
        }
    }
}

#[derive(Debug)]
pub enum TypeError {
    Somewhere(SomewhereTypeError),
    LocalizedError(LocalizedError),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Typable<'a> {
    Expr(&'a Expr),
    Stmt(&'a Stmt),
    Definition(&'a Token),
}

impl<'a> From<&'a Expr> for Typable<'a> {
    fn from(e: &'a Expr) -> Self {
        Typable::Expr(e)
    }
}

impl<'a> From<&'a Stmt> for Typable<'a> {
    fn from(s: &'a Stmt) -> Self {
        Typable::Stmt(s)
    }
}

impl<'a> From<&'a Token> for Typable<'a> {
    fn from(t: &'a Token) -> Self {
        Typable::Definition(t)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Typemap<'a>(HashMap<Typable<'a>, Type>);

impl<'a> Typemap<'a> {
    pub fn type_of(&self, obj: Typable) -> Type {
        self.0.get(&obj).cloned().unwrap_or_default()
    }

    pub(super) fn add_expr(&mut self, expr: &'a Expr, expr_type: Type) {
        self.0.insert(Typable::Expr(expr), expr_type);
    }

    pub(super) fn add_stmt(&mut self, stmt: &'a Stmt, stmt_type: Type) {
        self.0.insert(Typable::Stmt(stmt), stmt_type);
    }

    pub(super) fn add_definition(&mut self, name: &'a Token, def_type: Type) {
        self.0.insert(name.into(), def_type);
    }
}

pub struct Checker<'an, 'ast> {
    annotations: &'an Annotations,
    type_map: Typemap<'ast>,
}

impl<'a, 'ast> Checker<'a, 'ast> {
    pub fn typecheck(
        program: &'ast Program,
        annotations: &'a Annotations,
    ) -> Result<Typemap<'ast>, TypeError> {
        let mut checker = Checker::new(annotations);

        checker.perform_block_predef(program)?;

        for stmt in program {
            checker.visit_stmt(stmt)?;
        }

        Ok(checker.type_map)
    }

    pub fn new(annotations: &'a Annotations) -> Checker<'a, 'ast> {
        Self {
            annotations,
            type_map: Default::default(),
        }
    }

    fn check_expectation(provided: &Type, expected: &Type) -> Result<(), SomewhereTypeError> {
        if PartialOrd::le(provided, expected) {
            Ok(())
        } else {
            Err(SomewhereTypeError::TypeMismatch {
                expected: expected.clone(),
                got: provided.clone(),
            })
        }
    }

    fn lookup_type(&self, type_name: &TypeMention) -> Result<Type, TypeError> {
        TypeBuilder::build_type(self.annotations, &self.type_map, type_name)
            .map_err(|e| e.at(type_name.get_pos()).into())
    }

    fn lookup_type_of(&self, name: &TypedName) -> Result<Type, TypeError> {
        name.type_name
            .as_ref()
            .map(|t| self.lookup_type(t))
            .unwrap_or(Ok(Type::Unspecified))
    }

    fn perform_block_predef(&mut self, statements: &'ast [Stmt]) -> Result<(), TypeError> {
        for stmt in statements {
            match stmt {
                Stmt::VarDeclaration(v, _) => {
                    let var_type = if let Some(type_name) = &v.type_name {
                        self.lookup_type(type_name)?
                    } else {
                        Type::Unspecified
                    };

                    self.type_map.add_definition(&v.name, var_type);
                }
                Stmt::FunctionDeclaration {
                    name,
                    args,
                    vararg,
                    body: _,
                    returns,
                } => {
                    let function_signature = self.build_function_type(
                        args,
                        vararg.as_ref(),
                        returns
                            .as_ref()
                            .map(|ret| self.lookup_type(ret))
                            .transpose()?
                            .unwrap_or(Type::Unspecified),
                    )?;
                    self.type_map.add_definition(name, function_signature);
                }
                Stmt::StructDeclaration { name, fields } => {}
                Stmt::EnumDeclaration { name, variants } => {}
                Stmt::ImplBlock { .. } => {
                    //TODO impl binding
                }
                Stmt::Import { .. } => {
                    //nothing for now
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn build_function_type(
        &self,
        args: &'ast [TypedName],
        vararg: Option<&'ast TypedName>,
        returns: Type,
    ) -> Result<Type, TypeError> {
        let arg_type = args
            .iter()
            .map(|arg| self.lookup_type_of(arg))
            .collect::<Result<Vec<_>, _>>()?;

        let vararg = vararg
            .as_ref()
            .map(|v| self.lookup_type_of(v))
            .transpose()?;

        Ok(Type::build_function(arg_type, vararg, returns))
    }
}

impl<'a, 'ast> Visitor<'ast, Type, TypeError> for Checker<'a, 'ast> {
    fn after_stmt(&mut self, stmt: &'ast Stmt, value: Type) -> Result<Type, TypeError> {
        self.type_map.add_stmt(stmt, value.clone());
        Ok(value)
    }

    fn visit_var_stmt(
        &mut self,
        variable_name: &'ast TypedName,
        rhs: Option<&'ast Expr>,
    ) -> Result<Type, TypeError> {
        if rhs.is_some() {
            let t = self.visit_expr(rhs.unwrap())?;
            Self::check_expectation(&t, &self.lookup_type_of(variable_name)?)?;
        }
        Ok(Type::Nothing)
    }

    fn visit_assignment_stmt(
        &mut self,
        target: &'ast Token,
        value: &'ast Expr,
    ) -> Result<Type, TypeError> {
        let value = self.visit_expr(value)?;
        let definition_type = self
            .annotations
            .get_definiton(target)
            .map(|def| self.type_map.type_of(def.into()))
            .unwrap_or(Type::Unspecified);

        Self::check_expectation(&value, &definition_type)?;

        Ok(Type::Nothing)
    }

    fn visit_expr_stmt(&mut self, expr: &'ast Expr) -> Result<Type, TypeError> {
        self.visit_expr(expr)
    }

    fn visit_assert_statement(
        &mut self,
        keyword: &'ast Token,
        expr: &'ast Expr,
    ) -> Result<Type, TypeError> {
        let inner = self.visit_expr(expr)?;
        Self::check_expectation(&inner, &Type::Bool).map_err(|e| e.at(keyword.position))?;
        Ok(Type::Nothing)
    }

    fn visit_pass_stmt(&mut self, _keyword: &'ast Token) -> Result<Type, TypeError> {
        Ok(Type::Nothing)
    }

    fn visit_function_declaration_statement(
        &mut self,
        _name: &'ast Token,
        args: &'ast [TypedName],
        vararg: Option<&'ast TypedName>,
        body: &'ast Expr,
        returns: Option<&'ast TypeMention>,
    ) -> Result<Type, TypeError> {
        for arg in args.iter().chain(vararg.into_iter()) {
            self.type_map
                .add_definition(&arg.name, self.lookup_type_of(arg)?);
        }
        let provided_return = self.visit_expr(body)?;
        Self::check_expectation(
            &provided_return,
            &returns
                .map(|t| self.lookup_type(t))
                .transpose()?
                .unwrap_or_default(),
        )?;
        Ok(Type::Nothing)
    }

    fn visit_method(
        &mut self,
        name: &'ast Token,
        args: &'ast [TypedName],
        vararg: Option<&'ast TypedName>,
        body: &'ast Expr,
        returns: Option<&'ast TypeMention>,
    ) -> Result<Type, TypeError> {
        self.visit_function_declaration_statement(name, args, vararg, body, returns)
    }

    fn visit_struct_declaration_statement(
        &mut self,
        name: &'ast Token,
        fields: &[TypedName],
    ) -> Result<Type, TypeError> {
        Ok(Type::Nothing)
    }

    fn visit_enum_declaration(
        &mut self,
        name: &'ast Token,
        variants: &'ast [crate::parsing::ast::EnumVariant],
    ) -> Result<Type, TypeError> {
        Ok(Type::Nothing)
    }

    fn visit_property_assignment(
        &mut self,
        target: &'ast Expr,
        value: &'ast Expr,
    ) -> Result<Type, TypeError> {
        self.visit_expr(target)?;
        self.visit_expr(value)?;
        Ok(Type::Nothing)
    }

    fn visit_impl_block(
        &mut self,
        name: &'ast Token,
        implementations: &'ast [Stmt],
    ) -> Result<Type, TypeError> {
        implementations
            .iter()
            .try_for_each(|f| self.visit_stmt(f).map(|_| ()))?;

        Ok(Type::Nothing)
    }

    fn visit_import_stmt(
        &mut self,
        module: &'ast [Token],
        name: &'ast Token,
        rename: Option<&'ast Token>,
    ) -> Result<Type, TypeError> {
        Ok(Type::Nothing)
    }

    fn after_expr(&mut self, expr: &'ast Expr, value: Type) -> Result<Type, TypeError> {
        self.type_map.add_expr(expr, value.clone());
        Ok(value)
    }

    fn visit_bool_expr(&mut self, _token: &Token) -> Result<Type, TypeError> {
        Ok(Type::Bool)
    }

    fn visit_number_expr(&mut self, _token: &Token) -> Result<Type, TypeError> {
        Ok(Type::Int)
    }

    fn visit_float_number_expr(&mut self, _token: &Token) -> Result<Type, TypeError> {
        Ok(Type::Float)
    }

    fn visit_variable_expr(&mut self, variable_name: &'ast Token) -> Result<Type, TypeError> {
        Ok(self
            .annotations
            .get_definiton(variable_name)
            .map(|d| self.type_map.type_of(d.into()))
            .unwrap_or_default())
    }

    fn visit_string_expr(&mut self, _string_literal: &'ast Token) -> Result<Type, TypeError> {
        Ok(Type::String)
    }

    fn visit_binary_expr(
        &mut self,
        op: &Token,
        left: &'ast Expr,
        right: &'ast Expr,
    ) -> Result<Type, TypeError> {
        let left = self.visit_expr(left)?;
        let right = self.visit_expr(right)?;

        use TokenKind::*;

        macro_rules! num {
            ($e: pat) => {
                (_, $e, _)
            };
        }
        match (&left, &op.kind, &right) {
            num!(CompareEquals) | num!(CompareNotEquals) => Ok(Type::Bool), //always

            _ if left.is_unspecified() => Ok(Type::Unspecified),
            _ if right.is_unspecified() => Ok(Type::Unspecified),
            (Type::Int, Mod, Type::Int) => Ok(Type::Int),
            (Type::Bool, Or, Type::Bool) => Ok(Type::Bool),
            (Type::Bool, And, Type::Bool) => Ok(Type::Bool),
            num!(Minus) | num!(Star) | num!(Slash) | num!(Power) => {
                number_upcast_binary_op(&left, &right).map_err(|e| e.at(op.position).into())
            }

            num!(Plus) => {
                if left == Type::String && right == Type::String {
                    Ok(Type::String)
                } else {
                    number_upcast_binary_op(&left, &right).map_err(|e| e.at(op.position).into())
                }
            }

            num!(CompareGreater)
            | num!(CompareGreaterEqual)
            | num!(CompareLess)
            | num!(CompareLessEqual) => {
                if left == Type::String && right == Type::String {
                    Ok(Type::Bool)
                } else {
                    number_upcast_binary_op(&left, &right)
                        .map_err(|e| e.at(op.position).into())
                        .map(|_| Type::Bool)
                }
            }

            (left, Or | And, right) => Ok(Type::build_union(left.clone(), right.clone())),

            (left, _op, right) => Err(SomewhereTypeError::UnspecifiedBinary {
                left: left.clone(),
                op: op.clone(),
                right: right.clone(),
            }
            .at(op.position)
            .into()),
        }
    }

    fn visit_unary_expr(&mut self, op: &'ast Token, arg: &'ast Expr) -> Result<Type, TypeError> {
        let t = self.visit_expr(arg)?;

        match (&op.kind, t) {
            (TokenKind::Not, _) => Ok(Type::Bool),
            (_, Type::Unspecified) => Ok(Type::Unspecified),
            _ => unimplemented!(),
        }
    }

    fn visit_cond_expr(
        &mut self,
        condition: &'ast Expr,
        then_branch: &'ast Expr,
        else_branch: Option<&'ast Expr>,
    ) -> Result<Type, TypeError> {
        let condition_t = self.visit_expr(condition)?;

        Self::check_expectation(&condition_t, &Type::Bool)
            .map_err(|e| e.at(condition.get_pos()))?;

        let left = self.visit_expr(then_branch)?;
        let right = if let Some(else_branch) = else_branch {
            self.visit_expr(else_branch)?
        } else {
            Type::Nothing
        };

        Ok(Type::build_union(left, right))
    }

    fn visit_block(
        &mut self,
        _start_token: &Token,
        _end_token: &Token,
        containing_statements: &'ast [Stmt],
    ) -> Result<Type, TypeError> {
        self.perform_block_predef(containing_statements)?;

        let (last, rest) = containing_statements.split_last().unwrap();

        for stmt in rest {
            let _ = self.visit_stmt(stmt)?;
        }

        self.visit_stmt(last)
    }

    fn visit_single_statement_expr(&mut self, stmt: &'ast Stmt) -> Result<Type, TypeError> {
        self.visit_stmt(stmt)
    }

    fn visit_call_expr(
        &mut self,
        target: &'ast Expr,
        args: &'ast [Expr],
    ) -> Result<Type, TypeError> {
        let target_t = self.visit_expr(target)?;

        let args = args
            .iter()
            .map(|arg| self.visit_expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let (req_args, ret): (Vec<Type>, Type) = match () {
            _ if target_t.is_unspecified() => return Ok(Default::default()),
            _ if target_t.get_arity().is_none() => {
                return Err(SomewhereTypeError::OperationUnsupported {
                    target: target_t.clone(),
                    message: "cannot call".to_string(),
                }
                .at(target.get_pos())
                .into())
            }
            _ => {
                let arity = target_t.get_arity().unwrap();
                if !arity.accepts(args.len()) {
                    return Err(SomewhereTypeError::ArityMismatch {
                        expected: arity,
                        got: args.len(),
                    }
                    .at(target.get_pos())
                    .into());
                } else {
                    match target_t {
                        Type::StructDescriptor(_) => return Ok(Default::default()),
                        Type::Callable(c) => {
                            if c.vararg.is_some() {
                                let pad = args.len() - c.arguments.len();
                                (
                                    c.arguments
                                        .into_iter()
                                        .chain(std::iter::repeat(*c.vararg.unwrap()).take(pad))
                                        .collect::<Vec<_>>(),
                                    *c.return_type,
                                )
                            } else {
                                (c.arguments, *c.return_type)
                            }
                        }
                        Type::Union(_) => return Ok(Default::default()),
                        _ => unreachable!(),
                    }
                }
            }
        };

        req_args
            .iter()
            .zip(args.iter())
            .try_for_each(|(expected, provided)| Self::check_expectation(provided, expected))?;
        Ok(ret)
    }

    fn visit_partial_call_expr(
        &mut self,
        target: &'ast Expr,
        args: &'ast [Option<Expr>],
    ) -> Result<Type, TypeError> {
        self.visit_expr(target)?;
        for arg in args {
            if arg.is_some() {
                self.visit_expr(arg.as_ref().unwrap())?;
            }
        }
        Ok(Default::default())
    }

    fn visit_anon_function_expr(
        &mut self,
        args: &'ast [TypedName],
        vararg: Option<&'ast TypedName>,
        _arrow: &'ast Token,
        body: &'ast Expr,
    ) -> Result<Type, TypeError> {
        for arg in args.iter().chain(vararg.into_iter()) {
            self.type_map
                .add_definition(&arg.name, self.lookup_type_of(arg)?);
        }
        let ret = self.visit_expr(body)?;
        self.build_function_type(args, vararg, ret)
    }

    fn visit_property_access(
        &mut self,
        target: &'ast Expr,
        property: &'ast Token,
    ) -> Result<Type, TypeError> {
        self.visit_expr(target)?;
        Ok(Default::default())
    }

    fn visit_property_check(
        &mut self,
        target: &'ast Expr,
        _property: &'ast Token,
    ) -> Result<Type, TypeError> {
        self.visit_expr(target)?;
        Ok(Type::Bool)
    }
}

fn number_upcast_binary_op(left: &Type, right: &Type) -> Result<Type, SomewhereTypeError> {
    if !matches!(left, Type::Int | Type::Float) {
        return Err(SomewhereTypeError::TypeMismatch {
            expected: Type::build_union(Type::Int, Type::Float),
            got: left.clone(),
        });
    }
    if !matches!(right, Type::Int | Type::Float) {
        return Err(SomewhereTypeError::TypeMismatch {
            expected: Type::build_union(Type::Int, Type::Float),
            got: right.clone(),
        });
    }
    if matches!(left, Type::Int) && matches!(right, Type::Int) {
        Ok(Type::Int)
    } else {
        Ok(Type::Float)
    }
}

#[cfg(test)]
mod tests {

    use crate::{
        compile::{checks::Annotations, typecheck::types::Type},
        parsing::ast::{Expr, Stmt},
    };

    lazy_static! {
        static ref EMPTY_ANNOTATIONS: Annotations = Default::default();
    }

    fn make_expr(content: &str) -> Expr {
        use crate::parsing::lexer::tokenize;
        use crate::parsing::parser::program_parser;

        let tokens = tokenize(content).unwrap();

        program_parser::expr(tokens.iter().collect::<Vec<_>>().as_slice()).unwrap()
    }

    fn wrap_in_stmt(expr: Expr) -> Stmt {
        Stmt::Expression(expr)
    }

    use super::Checker;

    fn type_expected_expr(content: &str, expected_type: Type) {
        let ast = make_expr(content);

        let program = wrap_in_stmt(ast.clone());
        let program = &vec![program];
        let types = Checker::typecheck(program, &EMPTY_ANNOTATIONS).unwrap();

        assert_eq!(types.type_of((&ast).into()), expected_type);
    }

    fn error_expected_expr(content: &str) {
        let ast = make_expr(content);

        let program = wrap_in_stmt(ast);
        Checker::typecheck(&vec![program], &EMPTY_ANNOTATIONS).unwrap_err();
    }

    fn type_program(content: &str) {
        use crate::parsing::lexer::tokenize;
        use crate::parsing::parser::program_parser;

        let content = crate::execution::module::normalize_string(content);

        let tokens = tokenize(&content).unwrap();

        let program = program_parser::program(tokens.iter().collect::<Vec<_>>().as_slice())
            .map_err(|e| println!("{:?}\n{:?}", e, tokens[e.location]))
            .unwrap();

        let (program, annotations) = crate::compile::checks::check_optimize(program).unwrap();

        let _ = Checker::typecheck(&program, &annotations).unwrap();
    }

    fn error_program(content: &str) {
        use crate::parsing::lexer::tokenize;
        use crate::parsing::parser::program_parser;

        let content = crate::execution::module::normalize_string(content);

        let tokens = tokenize(&content).unwrap();

        let program = program_parser::program(tokens.iter().collect::<Vec<_>>().as_slice())
            .map_err(|e| println!("{:?}\n{:?}", e, tokens[e.location]))
            .unwrap();

        let (program, annotations) = crate::compile::checks::check_optimize(program).unwrap();

        let _ = Checker::typecheck(&program, &annotations).unwrap_err();
    }

    #[test]
    fn number_should_have_type_int() {
        type_expected_expr("1", Type::Int)
    }

    #[test]
    fn addition_of_ints_should_be_int() {
        type_expected_expr("1+2", Type::Int)
    }

    #[test]
    fn addition_of_int_and_float_should_be_float() {
        type_expected_expr("1+2.0", Type::Float);
        type_expected_expr("1.0+2", Type::Float);
        type_expected_expr("1.0 + 2.0", Type::Float);
    }

    #[test]
    fn addition_of_int_and_other_should_error() {
        error_expected_expr("1+true");
        error_expected_expr("true+1");
    }

    #[test]
    fn mod_should_allow_only_ints() {
        type_expected_expr("1 mod 2", Type::Int);
        error_expected_expr("1 mod true");
        error_expected_expr("1.0 mod 1");
    }
    #[test]
    fn comparisons() {
        type_expected_expr("1==2", Type::Bool);
        type_expected_expr("\"abc\"==1", Type::Bool);

        type_expected_expr("1!=2", Type::Bool);
        type_expected_expr("1!=\"abc\"", Type::Bool);

        type_expected_expr("1>2", Type::Bool);
        type_expected_expr("1>=2", Type::Bool);
        type_expected_expr("1<2", Type::Bool);
        type_expected_expr("1<=2", Type::Bool);

        error_expected_expr("true > false");
    }

    #[test]
    fn if_typecheck() {
        error_expected_expr("if 1 2 else 3"); // only bool
        type_expected_expr("if 1==2 2 else 3", Type::Int);
        type_expected_expr("if 1==2 2", Type::build_union(Type::Int, Type::Nothing));
        type_expected_expr(
            "if 1==2 2 else true",
            Type::build_union(Type::Int, Type::Bool),
        );
    }

    #[test]
    fn variable_assignment() {
        type_program(
            "
        var a: Int = 2
        a = 3
        ",
        );
        type_program(
            "
        var a = 2
        a = 3
        ",
        );
        error_program(
            //it catches errors
            "
        var a: Int = 2
        a = true
        ",
        );
        type_program(
            //it does not limits user too much
            "
        var a = 2
        a = true
        ",
        );
    }

    #[test]
    fn def_calling() {
        type_program(
            "
                def a =
                    0
                a() + 1
                ",
        );

        type_program(
            r"
        def a(x: Int):Int =
            x

        a(1) + 1
        ",
        );

        error_program(
            r"
def a(x: Int): Int =
    x
a(true)  # arguments are matched
",
        );

        error_program(
            r#"
def a(x: Int): Int =
    x
a(1) + "abcd"  # return value is matched   
"#,
        );
        error_program(
            r"
def a(x, y) =
    x+y
a(1)  #arity is checked    
",
        );

        error_program(
            r"
def a:Int = # return type is checked
    false
",
        );
    }

    #[test]
    fn anon_functions() {
        type_program(
            r"
((x) => x+1)(1)
",
        );
        error_program(
            r"
((x) => x+1)()  #arity is checked
",
        );
        error_program(
            r"

((x:Int) => x+1)(2) + true # return type is checked
",
        );
        error_program(
            r"
((x:Int) => x+1)(true) #args are checked
",
        )
    }

    #[test]
    fn unary_operator() {
        type_program(
            r"
assert not 0  # assert requires bool
            ",
        );
    }

    #[test]
    fn function_type() {
        type_program(
            r"
def F(op: Fn()=>Int):Int =
    op()

F(() => 2)
    ",
        );

        error_program(
            r"
def F(op: Fn()=>Int):Int =
    op()

F(1)
",
        );
        error_program(
            r"
def F(op: Fn()=>Int):Int =
    op()

F((x) => x+1)
",
        )
    }
}
