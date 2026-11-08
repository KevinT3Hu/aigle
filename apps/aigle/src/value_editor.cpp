#include "gui_json.hpp"
#include "value_editor.hpp"
#include "adjustments.hpp"
#include "curve.hpp"
#include <QCheckBox>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDoubleSpinBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QScrollArea>
#include <QVBoxLayout>
#include <memory>

namespace aigle {
namespace {
QJsonObject resolve(QJsonObject schema, const QJsonObject& root) {
    const auto reference = schema.value("$ref").toString();
    if (!reference.startsWith("#/"))
        return schema;
    QJsonValue value = root;
    for (const auto& part : reference.mid(2).split('/'))
        value = value.toObject().value(part);
    return value.toObject();
}
QJsonValue initial(const QJsonObject& schema) {
    if (schema.contains("default"))
        return schema.value("default");
    if (schema.contains("const"))
        return schema.value("const");
    const auto type = schema.value("type").toString();
    if (type == "object")
        return QJsonObject{};
    if (type == "array") {
        QJsonArray array;
        for (int i = 0; i < schema.value("minItems").toInt(); ++i)
            array.append(initial(schema.value("items").toObject()));
        return array;
    }
    if (type == "boolean")
        return false;
    if (type == "string")
        return schema.value("enum").toArray().isEmpty() ? QJsonValue("")
                                                        : schema.value("enum").toArray().first();
    if (type == "number" || type == "integer")
        return schema.value("minimum").toDouble(0);
    return QJsonValue::Null;
}
QString summary(const QJsonValue& value, int index) {
    const auto object = value.toObject();
    if (object.contains("name"))
        return object.value("name").toString();
    if (object.contains("kind"))
        return title(object.value("kind").toString());
    if (value.isString())
        return value.toString();
    return QObject::tr("Item %1").arg(index + 1);
}
} // namespace
QJsonObject schemaAt(const QJsonObject& root, const QString& path) {
    auto node = root;
    for (const auto& part : path.split('.')) {
        node = resolve(node, root);
        node = node.value("properties").toObject().value(part).toObject();
    }
    node = resolve(node, root);
    if (path.startsWith("curve.point."))
        node.insert("x-aigle-curve", true);
    return node;
}
bool editValue(QWidget* parent, const QString& name, const QJsonObject& schema, QJsonValue& value,
               const QJsonObject& root) {
    QDialog dialog(parent);
    dialog.setWindowTitle(name);
    dialog.resize(520, 500);
    auto* layout = new QVBoxLayout(&dialog);
    auto* scroll = new QScrollArea(&dialog);
    scroll->setWidgetResizable(true);
    auto* editor = new ValueEditor(schema, value, root);
    scroll->setWidget(editor);
    layout->addWidget(scroll);
    auto* buttons =
        new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel, &dialog);
    layout->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    if (dialog.exec() != QDialog::Accepted)
        return false;
    value = editor->value();
    return true;
}
ValueEditor::ValueEditor(QJsonObject schema, QJsonValue value, QJsonObject root, QWidget* parent)
    : QWidget(parent) {
    schema = resolve(schema, root);
    const auto types = schema.value("type").toArray();
    if (!types.isEmpty()) {
        QJsonArray alternatives;
        for (const auto& type : types) {
            auto variant = schema;
            variant.insert("type", type);
            alternatives.append(variant);
        }
        schema.remove("type");
        schema.insert("anyOf", alternatives);
    }
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(4, 4, 4, 4);
    const auto variants = schema.contains("oneOf") ? schema.value("oneOf").toArray()
                                                   : schema.value("anyOf").toArray();
    if (!variants.isEmpty()) {
        auto* choice = new QComboBox(this);
        layout->addWidget(choice);
        auto* body = new QWidget(this);
        auto* bodyLayout = new QVBoxLayout(body);
        bodyLayout->setContentsMargins(0, 0, 0, 0);
        layout->addWidget(body);
        auto current = std::make_shared<QPointer<ValueEditor>>();
        int selected = 0;
        for (qsizetype i = 0; i < variants.size(); ++i) {
            const auto variant = resolve(variants[i].toObject(), root);
            const auto kind =
                variant.value("properties").toObject().value("kind").toObject().value("const");
            QString label =
                kind.isString() ? title(kind.toString()) : title(variant.value("type").toString());
            if (label == "Null")
                label = tr("None");
            choice->addItem(label.isEmpty() ? tr("Option %1").arg(i + 1) : label);
            if ((!kind.isUndefined() && value.toObject().value("kind") == kind) ||
                (variant.value("type") == "null" && value.isNull()) ||
                (variant.value("type") == "object" && value.isObject() && kind.isUndefined()))
                selected = int(i);
        }
        auto rebuild = [current, body, bodyLayout, variants, root](int index, QJsonValue v) {
            if (*current)
                delete current->data();
            auto* editor = new ValueEditor(variants[index].toObject(), v, root, body);
            *current = editor;
            bodyLayout->addWidget(editor);
        };
        choice->setCurrentIndex(selected);
        rebuild(selected, value);
        connect(choice, qOverload<int>(&QComboBox::activated), this,
                [rebuild, variants, root](int index) {
                    rebuild(index, initial(resolve(variants[index].toObject(), root)));
                });
        read_ = [current] { return *current ? (*current)->value() : QJsonValue(QJsonValue::Null); };
        return;
    }
    if (schema.contains("const")) {
        const auto fixed = schema.value("const");
        layout->addWidget(new QLabel(title(fixed.toString()), this));
        read_ = [fixed] { return fixed; };
        return;
    }
    const auto type = schema.value("type").toString();
    if (type == "null") {
        layout->addWidget(new QLabel(tr("None"), this));
        read_ = [] { return QJsonValue(QJsonValue::Null); };
        return;
    }
    if (type == "object" || schema.contains("properties")) {
        auto* form = new QFormLayout;
        layout->addLayout(form);
        auto children = std::make_shared<std::vector<std::pair<QString, ValueEditor*>>>();
        const auto properties = schema.value("properties").toObject();
        for (auto it = properties.begin(); it != properties.end(); ++it) {
            const auto childSchema = resolve(it.value().toObject(), root);
            const auto childValue = value.toObject().contains(it.key())
                                        ? value.toObject().value(it.key())
                                        : initial(childSchema);
            auto* child = new ValueEditor(childSchema, childValue, root, this);
            child->setToolTip(childSchema.value("description").toString());
            form->addRow(title(it.key()), child);
            children->emplace_back(it.key(), child);
        }
        read_ = [children] {
            QJsonObject object;
            for (const auto& [key, child] : *children)
                object.insert(key, child->value());
            return object;
        };
        return;
    }
    if (type == "array") {
        const auto properties =
            resolve(schema.value("items").toObject(), root).value("properties").toObject();
        if (schema.value("x-aigle-curve").toBool() && properties.contains("x") &&
            properties.contains("y")) {
            auto* curve = new Curve(curvePointsFromJson(value.toArray()), this);
            layout->addWidget(curve);
            read_ = [curve] { return toJson(curve->points()); };
            return;
        }
        auto items = std::make_shared<QJsonArray>(value.toArray());
        auto* list = new QListWidget(this);
        layout->addWidget(list);
        auto refresh = [items, list] {
            list->clear();
            for (qsizetype i = 0; i < items->size(); ++i)
                list->addItem(summary(items->at(i), int(i)));
        };
        refresh();
        const auto itemSchema = schema.value("items").toObject();
        auto* buttons = new QWidget(this);
        auto* row = new QHBoxLayout(buttons);
        row->setContentsMargins(0, 0, 0, 0);
        layout->addWidget(buttons);
        auto *add = new QPushButton(tr("Add"), buttons),
             *edit = new QPushButton(tr("Edit"), buttons),
             *remove = new QPushButton(tr("Remove"), buttons);
        row->addWidget(add);
        row->addWidget(edit);
        row->addWidget(remove);
        connect(add, &QPushButton::clicked, this, [this, items, itemSchema, root, refresh] {
            auto item = initial(resolve(itemSchema, root));
            if (editValue(this, tr("Add item"), itemSchema, item, root)) {
                items->append(item);
                refresh();
            }
        });
        auto editItem = [this, items, list, itemSchema, root, refresh] {
            const auto index = list->currentRow();
            if (index < 0)
                return;
            auto item = items->at(index);
            if (editValue(this, tr("Edit item"), itemSchema, item, root)) {
                (*items)[index] = item;
                refresh();
                list->setCurrentRow(index);
            }
        };
        connect(edit, &QPushButton::clicked, this, editItem);
        connect(list, &QListWidget::itemDoubleClicked, this,
                [editItem](QListWidgetItem*) { editItem(); });
        connect(remove, &QPushButton::clicked, this, [items, list, refresh] {
            const auto index = list->currentRow();
            if (index >= 0) {
                items->removeAt(index);
                refresh();
            }
        });
        read_ = [items] { return *items; };
        return;
    }
    const auto choices = schema.value("enum").toArray();
    if (!choices.isEmpty()) {
        auto* combo = new QComboBox(this);
        for (const auto& choice : choices)
            combo->addItem(title(choice.toString()), choice.toVariant());
        combo->setCurrentIndex(std::max(0, combo->findData(value.toVariant())));
        layout->addWidget(combo);
        read_ = [combo] { return QJsonValue::fromVariant(combo->currentData()); };
        return;
    }
    if (type == "boolean" || value.isBool()) {
        auto* check = new QCheckBox(this);
        check->setChecked(value.toBool());
        layout->addWidget(check);
        read_ = [check] { return check->isChecked(); };
        return;
    }
    if (type == "integer" || type == "number" || value.isDouble()) {
        auto* spin = new QDoubleSpinBox(this);
        spin->setRange(schema.value("minimum").toDouble(-1e9),
                       schema.value("maximum").toDouble(1e9));
        spin->setDecimals(type == "integer" ? 0 : 6);
        spin->setSingleStep(schema.value("x-step").toDouble(0.01));
        spin->setValue(value.toDouble());
        layout->addWidget(spin);
        read_ = [spin] { return spin->value(); };
        return;
    }
    if (type == "null") {
        layout->addWidget(new QLabel(tr("None"), this));
        read_ = [] { return QJsonValue(QJsonValue::Null); };
        return;
    }
    auto* line = new QLineEdit(value.toString(), this);
    layout->addWidget(line);
    read_ = [line] { return line->text(); };
}
} // namespace aigle
