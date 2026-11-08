#include "gui_json.hpp"
#include "commands.hpp"
#include "adjustments.hpp"
#include "localization.hpp"
#include "curve.hpp"
#include "theme.hpp"
#include "value_editor.hpp"
#include <QCheckBox>
#include <QComboBox>
#include <QDoubleSpinBox>
#include <QFile>
#include <QFileDialog>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QScrollArea>
#include <QSignalBlocker>
#include <QSlider>
#include <QTabBar>
#include <QToolButton>
#include <cmath>

namespace aigle {
QString title(const QString& text) {
    if (text == "wb")
        return QObject::tr("White balance");
    if (text == "bw")
        return QObject::tr("Black & white");
    if (text == "hdr")
        return QStringLiteral("HDR");
    if (text == "curve")
        return QObject::tr("Tone curve");
    auto words = text.split('_');
    for (auto& word : words)
        if (!word.isEmpty())
            word[0] = word[0].toUpper();
    return uiText(words.join(' '));
}
QJsonValue valueAt(QJsonValue value, const QString& path) {
    for (const auto& part : path.split('.')) {
        value = value.isArray() ? value.toArray().at(part.toInt()) : value.toObject().value(part);
    }
    return value;
}
Adjustments::Adjustments(QWidget* parent) : QWidget(parent), tabs_(new QTabWidget(this)) {
    setObjectName("developPanel");
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    tabs_->setObjectName("developTabs");
    tabs_->setDocumentMode(true);
    auto* search = new QLineEdit(this);
    search->setObjectName("adjustSearch");
    bindTranslation(search, [this, search] {
        search->setPlaceholderText(tr("Find adjustment…"));
        search->setAccessibleName(tr("Find adjustment"));
    });
    auto* searchBox = new QWidget;
    auto* searchLayout = new QVBoxLayout(searchBox);
    searchLayout->setContentsMargins(16, 13, 16, 15);
    search->setFixedHeight(30);
    searchLayout->addWidget(search);
    layout->addWidget(searchBox);
    tabs_->tabBar()->hide();
    layout->addWidget(tabs_);
    connect(search, &QLineEdit::textChanged, this, [this](const QString& text) {
        tabs_->setCurrentIndex(0);
        for (auto* group : findChildren<QWidget*>("adjustmentGroup")) {
            QString content = group->property("searchText").toString();
            for (auto* label : group->findChildren<QLabel*>())
                content += ' ' + label->text();
            const bool match = content.contains(text, Qt::CaseInsensitive);
            group->setVisible(text.isEmpty() || match);
            if (!text.isEmpty() && match)
                group->findChild<QToolButton*>("groupHeader")->setChecked(true);
        }
    });
    const QStringList labels{tr("Adjust"), tr("Retouch"), tr("Masks"), tr("Info"), tr("Metadata")};
    for (int i = 0; i < labels.size(); ++i) {
        auto* body = new QWidget;
        if (i < 3) {
            pages_[size_t(i)] = new QVBoxLayout(body);
            pages_[size_t(i)]->setContentsMargins(16, 0, 16, 12);
            pages_[size_t(i)]->setSpacing(0);
        }
        auto* scroll = new QScrollArea;
        scroll->setWidgetResizable(true);
        scroll->setFrameShape(QFrame::NoFrame);
        scroll->setWidget(body);
        tabs_->addTab(scroll, labels[i]);
    }
    bindTranslation(tabs_, [this] {
        const QStringList translated{tr("Adjust"), tr("Retouch"), tr("Masks"), tr("Info"), tr("Metadata")};
        for (int i = 0; i < translated.size(); ++i) tabs_->setTabText(i, translated[i]);
    });
    // Opening an inspector never implicitly starts a drawing gesture.
}
void Adjustments::showPage(int page) {
    tabs_->setCurrentIndex(page);
}

void Adjustments::setAuxiliaryPages(QWidget* info, QWidget* metadata) {
    for (const auto& entry : std::array<std::pair<int, QWidget*>, 2>{{{3, info}, {4, metadata}}}) {
        auto* scroll = qobject_cast<QScrollArea*>(tabs_->widget(entry.first));
        auto* old = scroll->takeWidget();
        delete old;
        scroll->setWidget(entry.second);
    }
}
void Adjustments::setHistogram(const Histogram& histogram) {
    histogram_ = histogram;
    for (auto* curve : findChildren<Curve*>()) curve->setHistogram(histogram);
}
void Adjustments::setSchema(const QJsonObject& schema) {
    schema_ = schema;
    fields_.clear();
    sections_.clear();
    for (auto* layout : pages_)
        while (auto* item = layout->takeAt(0)) {
            delete item->widget();
            delete item;
        }
    QFile file(":/ui/assets/develop-layout.json");
    if (!file.open(QIODevice::ReadOnly))
        return;
    const auto groups = QJsonDocument::fromJson(file.readAll()).array();
    for (qsizetype i = 0; i < groups.size(); ++i) {
        const auto group = groups[i].toObject();
        auto* body = new QWidget;
        auto* content = new QVBoxLayout(body);
        content->setContentsMargins(0, 2, 0, 13);
        content->setSpacing(16);
        for (const auto& value : group.value("sections").toArray()) {
            const auto spec = value.toObject();
            auto* part = new QWidget(body);
            auto* layout = new QVBoxLayout(part);
            layout->setContentsMargins(0, 0, 0, 0);
            layout->setSpacing(0);
            auto* heading = new QHBoxLayout;
            auto* label = new QLabel(uiText(spec.value("title").toString()), part);
            bindTranslation(label, [label, source = spec.value("title").toString()] { label->setText(uiText(source)); });
            label->setObjectName("sectionCaption");
            heading->addWidget(label, 1);
            auto* reset = iconButton("reset", tr("Reset section"), part);
            reset->setObjectName("resetSection." + spec.value("extra").toString());
            bindTranslation(reset, [this, reset] { reset->setToolTip(tr("Reset section")); });
            reset->setFixedSize(18, 18);
            heading->addWidget(reset);
            if (spec.value("title").toString() == "Tone") {
                auto* automatic = new QPushButton(glyph("auto"), tr("Auto"));
                automatic->setFixedHeight(24);
                heading->addWidget(automatic);
                connect(automatic, &QPushButton::clicked, this,
                        [this] { emit command(commands::AutoTone{}); });
            }
            layout->addLayout(heading);
            const auto extra = spec.value("extra").toString();
            addExtra(layout, extra);
            QStringList paths;
            for (const auto& row : spec.value("rows").toArray()) {
                const auto field = row.toObject();
                const auto path = field.value("path").toString();
                paths.append(path);
                addField(layout, path, uiText(field.value("label").toString()),
                    [source = field.value("label").toString()] { return uiText(source); });
            }
            if (extra == "WhiteBalance")
                paths.append("wb.mode");
            if (extra == "Treatment")
                paths.append("bw.monochrome");
            if (extra == "PointCurve")
                paths.append("curve.point");
            if (extra == "CameraProfile")
                paths.append({"profile.camera", "profile.look"});
            if (extra == "RawSettings")
                paths.append({"raw.demosaic", "raw.highlight_recon", "raw.ca_correction", "raw.hot_pixels"});
            if (reset)
                connect(reset, &QToolButton::clicked, this, [this, paths] {
                    QList<EditOperation> ops;
                    for (const auto& path : paths)
                        ops.append(EditOperation{EditOperation::Kind::Reset, path, {}});
                    emit command(commands::Edit{.label = tr("Reset section"), .ops = ops});
                });
            if (reset)
                reset->hide();
            sections_.push_back({part, reset, paths});
            content->addWidget(part);
        }
        if (group.value("title") == "Optics & geometry") {
            addStructured(content, "geometry.crop", tr("Crop bounds…"), [this] { return tr("Crop bounds…"); });
            addStructured(content, "geometry.aspect", tr("Aspect ratio…"), [this] { return tr("Aspect ratio…"); });
        }
        auto* container = section(uiText(group.value("title").toString()), body, i == 0);
        bindTranslation(container, [container, source = group.value("title").toString()] {
            const auto text = uiText(source);
            container->setProperty("searchText", text);
            container->findChild<QToolButton*>("groupHeader")->setText(QString(text).replace("&", "&&"));
        });
        pages_[0]->addWidget(container);
    }
    auto* maskTools = new QHBoxLayout;
    for (const auto& item : std::array<std::pair<QString, QString>, 3>{
             {{"brush", tr("Brush")}, {"gradient", tr("Linear")}, {"view", tr("Radial")}}}) {
        auto* button = new QPushButton(glyph(item.first), item.second);
        maskTools->addWidget(button);
        const auto tool = item.first == "gradient" ? "linear"
                          : item.first == "view"   ? "radial"
                                                   : "brush";
        connect(button, &QPushButton::clicked, this, [this, tool] { emit toolSelected(tool); });
    }
    pages_[2]->addLayout(maskTools);
    auto* masks = new QListWidget;
    masks->setObjectName("maskList");
    masks->setMinimumHeight(100);
    pages_[2]->addWidget(masks);
    fields_.push_back({"masks", [masks](const QJsonValue& value) {
                           const auto selected = masks->currentRow();
                           masks->clear();
                           for (const auto& mask : value.toArray())
                               masks->addItem(mask.toObject().value("name").toString());
                           masks->setCurrentRow(selected);
                       }});
    connect(masks, &QListWidget::itemDoubleClicked, this,
            [this](QListWidgetItem*) { editJson("masks"); });
    addStructured(pages_[2], "masks", tr("Edit local adjustments…"), [this] { return tr("Edit local adjustments…"); });
    addStructured(pages_[1], "spots", tr("Edit healing and cloning spots…"), [this] { return tr("Edit healing and cloning spots…"); });
    addStructured(pages_[1], "red_eye", tr("Edit red-eye corrections…"), [this] { return tr("Edit red-eye corrections…"); });
    for (auto* page : pages_)
        page->addStretch();
    updateSourceControls();
}
void Adjustments::addExtra(QVBoxLayout* layout, const QString& extra) {
    if (extra == "WhiteBalance") {
        auto* row = new QHBoxLayout;
        auto* mode = new QComboBox;
        mode->addItem(tr("As shot"), "as_shot");
        mode->addItem(tr("Custom"), "custom");
        bindTranslation(mode, [this, mode] {
            const QSignalBlocker blocker(mode);
            mode->setItemText(0, tr("As shot"));
            mode->setItemText(1, tr("Custom"));
        });
        auto* automatic = new QPushButton(tr("Auto"));
        bindTranslation(automatic, [this, button = automatic] { button->setText(tr("Auto")); });
        automatic->setObjectName("secondary");
        auto* picker = iconButton("picker", tr("White balance picker"));
        mode->setFixedHeight(28);
        automatic->setFixedHeight(28);
        row->setSpacing(6);
        row->addWidget(mode, 1);
        row->addWidget(automatic);
        row->addWidget(picker);
        layout->addLayout(row);
        layout->addSpacing(8);
        connect(mode, qOverload<int>(&QComboBox::activated), this,
                [this, mode](int i) { edit("wb.mode", mode->itemData(i).toString()); });
        fields_.push_back({"wb.mode", [mode](const QJsonValue& value) {
                               const QSignalBlocker block(mode);
                               mode->setCurrentIndex(mode->findData(value.toString()));
                           }});
        connect(automatic, &QPushButton::clicked, this,
                [this] { emit command(commands::AutoWb{}); });
        connect(picker, &QToolButton::clicked, this, [this] { emit toolSelected("wb"); });
    } else if (extra == "CameraProfile") {
        for (const auto& kind : {QString("dcp"), QString("lut")}) {
            auto* import = new QPushButton(kind == "dcp" ? tr("Import camera profile…") : tr("Import look LUT…"));
            bindTranslation(import, [this, import, kind] {
                import->setText(kind == "dcp" ? tr("Import camera profile…") : tr("Import look LUT…"));
            });
            import->setObjectName("import." + kind);
            import->setProperty("cameraRawOnly", kind == "dcp");
            layout->addWidget(import);
            connect(import, &QPushButton::clicked, this, [this, kind] {
                const auto path = QFileDialog::getOpenFileName(this,
                    kind == "dcp" ? tr("Import camera profile") : tr("Import look LUT"), {},
                    kind == "dcp" ? tr("DNG camera profiles (*.dcp);;All files (*)") : tr("Cube LUTs (*.cube);;All files (*)"));
                if (!path.isEmpty()) emit command(commands::ImportProfile{.kind = kind, .path = path});
            });
            auto* status = new QLabel;
            status->setWordWrap(true);
            status->setProperty("cameraRawOnly", kind == "dcp");
            layout->addWidget(status);
            bindTranslation(status, [this, status, kind] {
                const auto value = valueAt(params_, kind == "dcp" ? "profile.camera" : "profile.look");
                status->setText(kind == "dcp" ? (value.toObject()["kind"] == "dcp" ? tr("Imported DCP profile") : tr("Standard camera matrix"))
                                             : (value.isNull() ? tr("No look LUT") : tr("Imported look LUT")));
            });
            fields_.push_back({kind == "dcp" ? "profile.camera" : "profile.look", [status, kind](const QJsonValue& value) {
                status->setText(kind == "dcp" ? (value.toObject()["kind"] == "dcp" ? tr("Imported DCP profile") : tr("Standard camera matrix"))
                                             : (value.isNull() ? tr("No look LUT") : tr("Imported look LUT")));
            }});
            auto* reset = new QPushButton(kind == "dcp" ? tr("Use standard camera matrix") : tr("Remove look LUT"));
            bindTranslation(reset, [this, reset, kind] {
                reset->setText(kind == "dcp" ? tr("Use standard camera matrix") : tr("Remove look LUT"));
            });
            reset->setProperty("cameraRawOnly", kind == "dcp");
            layout->addWidget(reset);
            connect(reset, &QPushButton::clicked, this, [this, kind] {
                edit(kind == "dcp" ? "profile.camera" : "profile.look", kind == "dcp" ? QJsonValue(QJsonObject{{"kind", "matrix"}, {"data", QJsonValue::Null}}) : QJsonValue(QJsonValue::Null));
            });
        }
    } else if (extra == "Treatment") {
        addField(layout, "bw.monochrome", tr("Black & white"), [this] { return tr("Black & white"); });
    } else if (extra == "LensProfile") {
        auto* match = new QPushButton(tr("Match camera and lens"));
        bindTranslation(match, [this, button = match] { button->setText(tr("Match camera and lens")); });
        match->setObjectName("matchLens");
        layout->addWidget(match);
        connect(match, &QPushButton::clicked, this, [this] { emit command(commands::MatchLens{}); });
        auto* status = new QLabel;
        layout->addWidget(status);
        bindTranslation(status, [this, status] {
            status->setText(valueAt(params_, "lens.profile").toObject()["kind"] == "resolved" ? tr("Matched lens profile enabled") : tr("Lens profile off"));
        });
        fields_.push_back({"lens.profile", [status](const QJsonValue& value) {
            status->setText(value.toObject()["kind"] == "resolved" ? tr("Matched lens profile enabled") : tr("Lens profile off"));
        }});
        auto* off = new QPushButton(tr("Disable lens profile"));
        bindTranslation(off, [this, button = off] { button->setText(tr("Disable lens profile")); });
        layout->addWidget(off);
        connect(off, &QPushButton::clicked, this, [this] { edit("lens.profile", QJsonObject{{"kind", "off"}}); });
    } else if (extra == "RawSettings") {
        auto* note = new QLabel;
        note->setProperty("rawOnlyNote", true);
        note->setWordWrap(true);
        bindTranslation(note, [this, note] { note->setText(tr("Decoding controls are available for RAW photos only.")); });
        layout->addWidget(note);
        auto* controls = new QWidget;
        controls->setObjectName("rawDecodingControls");
        controls->setProperty("rawDecodingOnly", true);
        auto* fields = new QVBoxLayout(controls);
        fields->setContentsMargins(0, 0, 0, 0);
        addFields(fields, schemaAt(schema_, "raw"), "raw");
        layout->addWidget(controls);
    } else if (extra == "PointCurve") {
        auto* channel = new QComboBox;
        for (const auto& key : {"rgb", "red", "green", "blue"})
            channel->addItem(QString::fromLatin1(key).toUpper(), QString::fromLatin1(key));
        channel->setObjectName("pointCurveChannel");
        bindTranslation(channel, [this, channel] {
            const QSignalBlocker blocker(channel);
            channel->setAccessibleName(tr("Curve channel"));
            const QStringList names{QStringLiteral("RGB"), tr("Red"), tr("Green"), tr("Blue")};
            for (int i = 0; i < names.size(); ++i) channel->setItemText(i, names[i]);
        });
        layout->addWidget(channel);
        auto* curve = new Curve({});
        curve->setHistogram(histogram_);
        curve->setMinimumSize(0, 170);
        curve->setMaximumHeight(200);
        layout->addWidget(curve);
        for (const auto& key : {"rgb", "red", "green", "blue"}) {
            const QString path = "curve.point." + QString::fromLatin1(key);
            fields_.push_back({path, [curve, channel, key](const QJsonValue& value) {
                                   if (channel->currentData().toString() ==
                                       QString::fromLatin1(key))
                                       curve->setPoints(curvePointsFromJson(value.toArray()));
                               }});
        }
        connect(channel, qOverload<int>(&QComboBox::activated), this, [this, curve, channel](int) {
            curve->setChannel(channel->currentData().toString());
            curve->setPoints(
                curvePointsFromJson(valueAt(params_, "curve.point." + channel->currentData().toString()).toArray()));
        });
        connect(curve, &Curve::dragStarted, this, [this, channel] {
            ++gesture_;
            channel->setEnabled(false);
        });
        connect(curve, &Curve::pointsPreviewed, this, [this, channel](const CurvePoints& points) {
            emit command(commands::Edit{.label = title(channel->currentData().toString()), .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Set, "curve.point." + channel->currentData().toString(), toJson(points)}}, .gesture = qint64(gesture_), .continuous = true});
        });
        connect(curve, &Curve::pointsChanged, this, [this, channel](const CurvePoints& points) {
            const bool dragging = !channel->isEnabled();
            edit("curve.point." + channel->currentData().toString(), toJson(points), dragging);
            channel->setEnabled(true);
        });
    }
}
void Adjustments::addFields(QVBoxLayout* layout, const QJsonObject& schema, const QString& prefix) {
    const auto properties = schema.value("properties").toObject();
    for (auto it = properties.begin(); it != properties.end(); ++it) {
        const auto path = prefix + '.' + it.key();
        const auto node = it.value().toObject();
        if (node.contains("properties"))
            addFields(layout, node, path);
        else
            addField(layout, path, title(it.key()), [source = it.key()] { return title(source); });
    }
}
void Adjustments::addStructured(QVBoxLayout* layout, const QString& path, const QString& label, std::function<QString()> translated) {
    auto* button = new QPushButton(label);
    if (translated) bindTranslation(button, [button, translated] { button->setText(translated()); });
    connect(button, &QPushButton::clicked, this, [this, path] { editJson(path); });
    layout->addWidget(button);
}
void Adjustments::addField(QVBoxLayout* layout, const QString& path, const QString& label, std::function<QString()> translated) {
    if (!translated) translated = [label] { return label; };
    const auto spec = schemaAt(schema_, path);
    const auto type = spec.value("type").toString();
    const auto values = spec.value("enum").toArray();
    if (type == "number" || type == "integer") {
        const double minimum = spec.value("minimum").toDouble(-100),
                     maximum = spec.value("maximum").toDouble(100),
                     step = spec.value("x-step").toDouble(type == "integer" ? 1 : 0.01);
        auto* field = new QWidget;
        auto* row = new QVBoxLayout(field);
        row->setContentsMargins(0, 4, 0, 5);
        row->setSpacing(0);
        auto* line = new QHBoxLayout;
        line->setContentsMargins(0, 0, 0, 0);
        auto* name = new QLabel(label);
        bindTranslation(name, [name, translated] { name->setText(translated()); });
        name->setObjectName("parameterLabel");
        line->addWidget(name, 1);
        auto* number = new QDoubleSpinBox;
        number->setObjectName("parameterValue");
        number->setProperty("parameterPath", path);
        bindTranslation(number, [number, translated] { number->setAccessibleName(translated()); });
        name->setBuddy(number);
        number->setButtonSymbols(QAbstractSpinBox::NoButtons);
        number->setAlignment(Qt::AlignRight);
        number->setFixedWidth(56);
        number->setFixedHeight(22);
        number->setRange(minimum, maximum);
        number->setSingleStep(step);
        number->setDecimals(
            type == "integer"
                ? 0
                : std::clamp(int(std::ceil(-std::log10(std::max(step, 1e-6)))), 0, 6));
        number->setKeyboardTracking(false);
        line->addWidget(number);
        const auto unit = spec.value("x-unit").toString();
        if (!unit.isEmpty()) {
            auto* unitLabel = new QLabel(unit);
            unitLabel->setObjectName("statusText");
            line->addWidget(unitLabel);
        }
        auto* reset = iconButton("reset", tr("Reset %1").arg(label));
        bindTranslation(reset, [this, reset, translated] { reset->setToolTip(tr("Reset %1").arg(translated())); });
        reset->setFixedSize(18, 18);
        reset->setIconSize({12, 12});
        line->addWidget(reset);
        connect(reset, &QToolButton::clicked, this, [this, path] {
            emit command(commands::Edit{.label = tr("Reset %1").arg(path), .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Reset, path, {}}}});
        });
        row->addLayout(line);
        auto* slider = new QSlider(Qt::Horizontal);
        slider->setObjectName(path);
        bindTranslation(slider, [slider, translated] { slider->setAccessibleName(translated()); });
        slider->setRange(0, 10000);
        slider->setFixedHeight(16);
        slider->setProperty("temperature", path == "wb.temperature");
        slider->setProperty("tint", path == "wb.tint");
        row->addWidget(slider);
        layout->addWidget(field);
        field->setToolTip(spec.value("description").toString());
        const bool logarithmic = path == "wb.temperature" && minimum > 0;
        auto toValue = [minimum, maximum, step, logarithmic](int position) {
            const auto t = position / 10000.0;
            const auto value = logarithmic
                                   ? std::exp(std::log(minimum) + t * std::log(maximum / minimum))
                                   : minimum + t * (maximum - minimum);
            return std::clamp(std::round(value / step) * step, minimum, maximum);
        };
        auto toPosition = [minimum, maximum, logarithmic](double value) {
            if (maximum <= minimum)
                return 0;
            const auto t = logarithmic ? std::log(std::max(value, minimum) / minimum) /
                                             std::log(maximum / minimum)
                                       : (value - minimum) / (maximum - minimum);
            return qRound(std::clamp(t, 0.0, 1.0) * 10000);
        };
        connect(slider, &QSlider::sliderPressed, this, [this] { ++gesture_; });
        connect(slider, &QSlider::valueChanged, this,
                [this, path, slider, number, toValue](int position) {
                    if (updating_)
                        return;
                    if (!slider->isSliderDown())
                        ++gesture_;
                    const auto value = toValue(position);
                    {
                        const QSignalBlocker block(number);
                        number->setValue(value);
                    }
                    QList<EditOperation> ops{EditOperation{EditOperation::Kind::Set, path, value}};
                    if ((path == "wb.temperature" || path == "wb.tint") &&
                        valueAt(params_, "wb.mode") == "as_shot") {
                        const QString companion =
                            path == "wb.temperature" ? "wb.tint" : "wb.temperature";
                        ops.append(EditOperation{EditOperation::Kind::Set, companion, displayed(companion)});
                    }
                    if (path.startsWith("wb."))
                        ops.append(
                            EditOperation{EditOperation::Kind::Set, "wb.mode", "custom"});
                    emit command(commands::Edit{.label = title(path.section('.', -1)), .ops = ops, .gesture = qint64(gesture_), .continuous = slider->isSliderDown()});
                });
        connect(slider, &QSlider::sliderReleased, this,
                [this, path, slider, toValue] { edit(path, toValue(slider->value()), true); });
        connect(number, qOverload<double>(&QDoubleSpinBox::valueChanged), this,
                [this, path, slider, toPosition](double value) {
                    if (updating_)
                        return;
                    const QSignalBlocker block(slider);
                    slider->setValue(toPosition(value));
                    ++gesture_;
                    edit(path, value, true);
                });
        fields_.push_back({path, [slider, number, toPosition](const QJsonValue& value) {
                               if (!slider->isSliderDown()) {
                                   const QSignalBlocker block(slider);
                                   slider->setValue(toPosition(value.toDouble()));
                               }
                               if (!number->hasFocus()) {
                                   const QSignalBlocker block(number);
                                   number->setValue(value.toDouble());
                               }
                           }});
    } else if (!values.isEmpty()) {
        auto* row = new QHBoxLayout;
        auto* name = new QLabel(label);
        bindTranslation(name, [name, translated] { name->setText(translated()); });
        row->addWidget(name);
        auto* combo = new QComboBox;
        combo->setObjectName(path);
        bindTranslation(combo, [combo, translated] { combo->setAccessibleName(translated()); });
        name->setBuddy(combo);
        for (const auto& value : values)
            combo->addItem(title(value.toString()), value.toVariant());
        bindTranslation(combo, [combo, values] {
            const QSignalBlocker blocker(combo);
            for (int i = 0; i < values.size(); ++i) combo->setItemText(i, title(values[i].toString()));
        });
        row->addWidget(combo, 1);
        layout->addLayout(row);
        connect(combo, qOverload<int>(&QComboBox::activated), this, [this, path, combo](int index) {
            edit(path, QJsonValue::fromVariant(combo->itemData(index)));
        });
        fields_.push_back({path, [combo](const QJsonValue& value) {
                               const QSignalBlocker block(combo);
                               combo->setCurrentIndex(combo->findData(value.toVariant()));
                           }});
    } else if (type == "boolean") {
        auto* check = new QCheckBox(label);
        bindTranslation(check, [check, translated] { check->setText(translated()); });
        layout->addWidget(check);
        connect(check, &QCheckBox::toggled, this, [this, path](bool on) { edit(path, on); });
        fields_.push_back({path, [check](const QJsonValue& value) {
                               const QSignalBlocker block(check);
                               check->setChecked(value.toBool());
                           }});
    } else
        addStructured(layout, path, label + QStringLiteral("…"), [translated] { return translated() + QStringLiteral("…"); });
}
QJsonValue Adjustments::displayed(const QString& path) const {
    if (valueAt(params_, "wb.mode") == "as_shot" &&
        (path == "wb.temperature" || path == "wb.tint")) {
        const auto value = path == "wb.temperature" ? info_.temperature : info_.tint;
        if (value)
            return *value;
    }
    return valueAt(params_, path);
}
void Adjustments::updateSourceControls() {
    const auto kind = info_.kind;
    const bool raw = kind == "Raw";
    const bool decoding = kind.isEmpty() || raw;
    for (auto* widget : findChildren<QWidget*>()) {
        if (widget->property("cameraRawOnly").toBool()) widget->setVisible(raw);
        if (widget->property("rawDecodingOnly").toBool()) widget->setVisible(decoding);
        if (widget->property("rawOnlyNote").toBool()) widget->setVisible(!decoding);
    }
}
void Adjustments::setImageInfo(const ImageInfo& info) {
    if (info_ == info)
        return;
    info_ = info;
    updateSourceControls();
    if (!params_.isEmpty())
        setParams(params_);
}
void Adjustments::setParams(const QJsonObject& params) {
    params_ = params;
    updating_ = true;
    for (const auto& field : fields_)
        field.update(displayed(field.path));
    updating_ = false;
    const bool monochrome = valueAt(params_, "bw.monochrome").toBool();
    for (const auto& part : sections_) {
        bool modified = false;
        for (const auto& path : part.paths) {
            const auto spec = schemaAt(schema_, path);
            if (spec.contains("default") && valueAt(params_, path) != spec.value("default"))
                modified = true;
        }
        if (part.reset)
            part.reset->setVisible(modified);
        if (!part.paths.isEmpty() && part.paths.first().startsWith("mixer."))
            part.body->setVisible(!monochrome);
        if (!part.paths.isEmpty() && part.paths.first().startsWith("bw.") &&
            part.paths.first() != "bw.monochrome")
            part.body->setVisible(monochrome);
    }
}
void Adjustments::edit(const QString& path, const QJsonValue& value, bool gesture) {
    if (updating_)
        return;
    QList<EditOperation> ops{EditOperation{EditOperation::Kind::Set, path, value}};
    if ((path == "wb.temperature" || path == "wb.tint") &&
        valueAt(params_, "wb.mode") == "as_shot") {
        const QString companion = path == "wb.temperature" ? "wb.tint" : "wb.temperature";
        ops.append(
            EditOperation{EditOperation::Kind::Set, companion, displayed(companion)});
    }
    if (path == "wb.temperature" || path == "wb.tint")
        ops.append(EditOperation{EditOperation::Kind::Set, "wb.mode", "custom"});
    auto request = commands::Edit{.label = title(path.section('.', -1)), .ops = ops};
    if (gesture)
        request.gesture = qint64(gesture_);
    emit command(request);
}
void Adjustments::editJson(const QString& path) {
    auto value = valueAt(params_, path);
    if (editValue(this, tr("Edit %1").arg(title(path)), schemaAt(schema_, path), value, schema_))
        edit(path, value);
}
} // namespace aigle
