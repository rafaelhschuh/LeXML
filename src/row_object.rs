use gtk::glib;
use gtk::subclass::prelude::*;
use std::cell::RefCell;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RowObject {
        pub rowid: RefCell<i64>,
        pub values: RefCell<Vec<String>>,
        pub editable: RefCell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RowObject {
        const NAME: &'static str = "LeXmlRowObject";
        type Type = super::RowObject;
    }

    impl ObjectImpl for RowObject {}
}

glib::wrapper! {
    pub struct RowObject(ObjectSubclass<imp::RowObject>);
}

impl RowObject {
    pub fn new(rowid: i64, values: Vec<String>, editable: bool) -> Self {
        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        *imp.rowid.borrow_mut() = rowid;
        *imp.values.borrow_mut() = values;
        *imp.editable.borrow_mut() = editable;
        obj
    }

    pub fn rowid(&self) -> i64 {
        *self.imp().rowid.borrow()
    }

    pub fn editable(&self) -> bool {
        *self.imp().editable.borrow()
    }

    pub fn value(&self, idx: usize) -> String {
        self.imp()
            .values
            .borrow()
            .get(idx)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_value(&self, idx: usize, val: String) {
        if let Some(slot) = self.imp().values.borrow_mut().get_mut(idx) {
            *slot = val;
        }
    }

    /// Acrescenta um valor ao final (usado ao adicionar uma coluna).
    pub fn push_value(&self, val: String) {
        self.imp().values.borrow_mut().push(val);
    }

    /// Remove o valor de índice `idx` (usado ao excluir uma coluna).
    pub fn remove_value(&self, idx: usize) {
        let mut v = self.imp().values.borrow_mut();
        if idx < v.len() {
            v.remove(idx);
        }
    }
}
