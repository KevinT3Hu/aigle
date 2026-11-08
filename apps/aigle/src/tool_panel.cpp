#include "commands.hpp"
#include "tool_panel.hpp"
#include "localization.hpp"
#include "adjustments.hpp"
#include "mask_delegate.hpp"
#include "theme.hpp"
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QGridLayout>
#include <QInputDialog>
#include <QLabel>
#include <QMessageBox>
#include <QPushButton>
#include <QScrollArea>
#include <QSignalBlocker>
#include <QSlider>
#include <QToolButton>
#include <cmath>

namespace aigle {
namespace {
QWidget* row(const char* source, double min, double max, double step, const QString& unit,
             std::function<void(double)> changed, QDoubleSpinBox** result = nullptr) {
    auto* box = new QWidget;
    auto* layout = new QVBoxLayout(box);
    layout->setContentsMargins(0, 4, 0, 5);
    layout->setSpacing(2);
    auto* line = new QHBoxLayout;
    line->setSpacing(4);
    const auto label = QCoreApplication::translate("aigle::ToolPanel", source);
    auto* name = new QLabel(label);
    name->setObjectName("parameterLabel");
    line->addWidget(name, 1);
    auto* number = new QDoubleSpinBox;
    number->setObjectName("parameterValue");
    number->setRange(min, max);
    number->setSingleStep(step);
    number->setDecimals(step < 1 ? 2 : 0);
    number->setButtonSymbols(QAbstractSpinBox::NoButtons);
    number->setAlignment(Qt::AlignRight);
    number->setFixedSize(56, 22);
    number->setKeyboardTracking(false);
    number->setAccessibleName(label);
    line->addWidget(number);
    auto* units = new QLabel(unit);
    units->setObjectName("statusText");
    line->addWidget(units);
    layout->addLayout(line);
    auto* slider = new QSlider(Qt::Horizontal);
    slider->setRange(0, int(std::round((max - min) / step)));
    slider->setFixedHeight(16);
    slider->setAccessibleName(label);
    bindTranslation(box, [name, number, slider, source] {
        const auto text = QCoreApplication::translate("aigle::ToolPanel", source);
        name->setText(text);
        number->setAccessibleName(text);
        slider->setAccessibleName(text);
    });
    layout->addWidget(slider);
    QObject::connect(slider, &QSlider::valueChanged, number, [number, min, step](int value) {
        const QSignalBlocker block(number);
        number->setValue(min + value * step);
    });
    QObject::connect(slider, &QSlider::sliderReleased, box,
                     [number, changed] { changed(number->value()); });
    QObject::connect(slider, &QSlider::actionTriggered, box,
                     [slider, number, min, step, changed](int) {
                         if (!slider->isSliderDown()) {
                             const double value = min + slider->sliderPosition() * step;
                             const QSignalBlocker block(number);
                             number->setValue(value);
                             changed(value);
                         }
                     });
    QObject::connect(number, qOverload<double>(&QDoubleSpinBox::valueChanged), box,
                     [slider, min, step, changed](double value) {
                         const QSignalBlocker block(slider);
                         slider->setValue(qRound((value - min) / step));
                         changed(value);
                     });
    number->setProperty("slider", QVariant::fromValue<QObject*>(slider));
    number->setProperty("minimum", min);
    number->setProperty("step", step);
    if (result)
        *result = number;
    return box;
}
void setNumber(QDoubleSpinBox* number, double value) {
    if (number->hasFocus())
        return;
    auto* slider = qobject_cast<QSlider*>(number->property("slider").value<QObject*>());
    if (slider->isSliderDown())
        return;
    const QSignalBlocker a(number), b(slider);
    number->setValue(value);
    slider->setValue(qRound((value - number->property("minimum").toDouble()) /
                            number->property("step").toDouble()));
}
} // namespace
ToolPanel::ToolPanel(QWidget* parent) : QWidget(parent) {
    auto* outer = new QVBoxLayout(this);
    outer->setContentsMargins(0, 0, 0, 0);
    auto* scroll = new QScrollArea;
    scroll->setWidgetResizable(true);
    auto* body = new QWidget;
    body_ = new QVBoxLayout(body);
    body_->setContentsMargins(16, 0, 16, 16);
    body_->setSpacing(8);
    scroll->setWidget(body);
    outer->addWidget(scroll);
}
void ToolPanel::setPage(const QString& page) {
    if (page_ == page && page != "crop")
        return;
    page_ = page;
    rebuild();
}
void ToolPanel::setGuides(const QList<MaskGuide>& guides) {
    guides_ = guides;
    if (page_ == "masks" && masks_)
        selectMask(selected_);
}
void ToolPanel::setParams(const QJsonObject& params) {
    params_ = params;
    if (page_ == "masks" && masks_) {
        updating_ = true;
        const auto values = params_["masks"].toArray();
        if (pendingMask_.has_value()) {
            for (int i = 0; i < values.size(); ++i) if (values[i].toObject()["id"].toInteger() == *pendingMask_) {
                selected_ = i;
                pendingMask_.reset();
                break;
            }
        }
        QStringList names;
        for (const auto& m : values) {
            const auto mask = m.toObject();
            names.append(mask["name"].toString() + "\n" +
                         title(mask["shape"].toObject()["kind"].toString()));
        }
        QStringList previous;
        for (int i = 0; i < masks_->count(); ++i)
            previous.append(masks_->item(i)->text());
        if (previous != names) {
            const QSignalBlocker block(masks_);
            masks_->clear();
            masks_->addItems(names);
            selected_ = std::clamp(selected_, 0, std::max(0, masks_->count() - 1));
            masks_->setCurrentRow(selected_);
        }
        for (int i = 0; i < masks_->count(); ++i) {
            const QJsonValue id = values[i].toObject()["id"];
            QImage swatch;
            for (const auto& value : guides_) {
                const auto& guide = value;
                if (guide.id != id.toInteger())
                    continue;
                const auto coverage = guide.coverage;
                if (coverage.size() == 32 * 32) {
                    swatch = QImage(32, 32, QImage::Format_Grayscale8);
                    for (int y = 0; y < 32; ++y)
                        for (int x = 0; x < 32; ++x)
                            swatch.scanLine(y)[x] = uchar(coverage[y * 32 + x]);
                }
                break;
            }
            masks_->item(i)->setData(Qt::DecorationRole, swatch);
            masks_->item(i)->setToolTip(
                swatch.isNull() ? tr("Coverage preview unavailable for image-dependent masks")
                                : tr("Mask coverage, including opacity and inversion"));
        }
        updating_ = false;
        selectMask(selected_);
    }
    for (const auto& f : sync_)
        f();
}
void ToolPanel::publishMasks(const QJsonArray& values, const QString& label) {
    if (updating_)
        return;
    params_["masks"] = values;
    emit command(
        commands::Edit{.label = label, .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Set, "masks", values}}});
}
void ToolPanel::changeMask(const QString& path, QJsonValue value) {
    auto values = params_["masks"].toArray();
    if (updating_ || selected_ < 0 || selected_ >= values.size())
        return;
    auto mask = values[selected_].toObject();
    if (path == "shape.range.low") value = std::min(value.toDouble(), valueAt(mask, "shape.range.high").toDouble());
    if (path == "shape.range.high") value = std::max(value.toDouble(), valueAt(mask, "shape.range.low").toDouble());
    const auto parts = path.split('.');
    const std::function<QJsonObject(QJsonObject, int)> set = [&](QJsonObject object, int index) {
        object[parts[index]] = index + 1 == parts.size() ? value : QJsonValue(set(object[parts[index]].toObject(), index + 1));
        return object;
    };
    mask = set(mask, 0);
    values[selected_] = mask;
    publishMasks(values, tr("Mask %1").arg(path));
}
void ToolPanel::selectMask(int rowIndex) {
    if (updating_)
        return;
    selected_ = rowIndex;
    const auto values = params_["masks"].toArray();
    if (rowIndex < 0 || rowIndex >= values.size()) {
        emit maskSelected({});
        return;
    }
    const auto mask = values[rowIndex].toObject();
    if (invert_) {
        const QSignalBlocker block(invert_);
        invert_->setChecked(mask["invert"].toBool());
    }
    for (const auto& f : sync_)
        f();
    emit maskSelected({});
    for (const auto& guide : guides_)
        if (guide.id == mask["id"].toInteger()) {
            emit maskSelected(guide.valid ? guide : MaskGuide{});
            break;
        }
    if (overlay_ && overlay_->isChecked())
        emit command(commands::Overlays{.overlays = OverlayPatch{.mask = std::optional<qint64>{mask["id"].toInteger()}}});
}
void ToolPanel::setCropAngle(double angle) {
    if (cropAngleEditor_)
        setNumber(cropAngleEditor_, angle);
}
void ToolPanel::rebuild() {
    cropAngleEditor_ = nullptr;
    sync_.clear();
    masks_ = nullptr;
    overlay_ = invert_ = nullptr;
    while (auto* item = body_->takeAt(0)) {
        if (item->widget())
            delete item->widget();
        delete item;
    }
    auto* heading = new QWidget;
    auto* hl = new QHBoxLayout(heading);
    hl->setContentsMargins(0, 10, 0, 10);
    auto* titleLabel = new QLabel(page_ == "crop" ? tr("Crop & straighten") : tr("Masks"));
    titleLabel->setObjectName("sectionTitle");
    hl->addWidget(titleLabel, 1);
    bindTranslation(titleLabel, [this, titleLabel, crop = page_ == "crop"] {
        titleLabel->setText(crop ? tr("Crop & straighten") : tr("Masks"));
    });
    auto* close = new QPushButton(page_ == "crop" ? tr("Cancel") : tr("Done"));
    bindTranslation(close, [this, close, crop = page_ == "crop"] { close->setText(crop ? tr("Cancel") : tr("Done")); });
    hl->addWidget(close);
    connect(close, &QPushButton::clicked, this, [this] {
        if (page_ == "crop")
            emit cropCancelled();
        emit done();
    });
    body_->addWidget(heading);
    if (page_ == "crop") {
        auto* line = new QWidget;
        auto* l = new QHBoxLayout(line);
        l->setContentsMargins(0, 0, 0, 0);
        auto* aspectLabel = new QLabel;
        bindTranslation(aspectLabel, [this, aspectLabel] { aspectLabel->setText(tr("Aspect ratio")); });
        l->addWidget(aspectLabel);
        auto* aspect = new QComboBox;
        aspect->setObjectName("cropAspect");
        aspect->addItems({tr("Original"), tr("Free"), tr("Square · 1 : 1"), tr("Wide · 16 : 9"),
                          tr("3 : 2"), tr("4 : 3"), tr("4 : 5")});
        bindTranslation(aspect, [this, aspect] {
            const QSignalBlocker blocker(aspect);
            const QStringList labels{tr("Original"), tr("Free"), tr("Square · 1 : 1"), tr("Wide · 16 : 9"), tr("3 : 2"), tr("4 : 3"), tr("4 : 5")};
            for (int i = 0; i < labels.size(); ++i) aspect->setItemText(i, labels[i]);
        });
        const std::array<double, 7> ratios{-1, 0, 1, 16. / 9, 1.5, 4. / 3, 4. / 5};
        for (int i = 0; i < int(ratios.size()); ++i) aspect->setItemData(i, ratios[size_t(i)]);
        const auto saved = valueAt(params_, "geometry.aspect").toObject();
        double ratio = saved["kind"] == "original" ? -1 : 0;
        if (saved["kind"] == "ratio" && saved["h"].toDouble() > 0)
            ratio = saved["w"].toDouble() / saved["h"].toDouble();
        int selected = -1;
        for (int i = 0; i < aspect->count(); ++i)
            if (std::abs(aspect->itemData(i).toDouble() - ratio) < 1e-9) selected = i;
        if (selected < 0) {
            aspect->addItem(QString("%1 : %2").arg(saved["w"].toInt()).arg(saved["h"].toInt()), ratio);
            selected = aspect->count() - 1;
        }
        aspect->setCurrentIndex(selected);
        l->addWidget(aspect, 1);
        body_->addWidget(line);
        connect(aspect, qOverload<int>(&QComboBox::activated), this, [this, aspect](int i) {
            emit aspectChanged(aspect->itemData(i).toDouble());
        });
        auto* lock = new QCheckBox(tr("Lock aspect ratio"));
        bindTranslation(lock, [this, widget = lock] { widget->setText(tr("Lock aspect ratio")); });
        lock->setChecked(ratio != 0);
        body_->addWidget(lock);
        connect(aspect, qOverload<int>(&QComboBox::activated), lock, [aspect, lock](int) {
            const QSignalBlocker blocker(lock);
            lock->setChecked(aspect->currentData().toDouble() != 0);
        });
        connect(lock, &QCheckBox::toggled, this, [this, aspect](bool on) {
            emit aspectChanged(on ? aspect->currentData().toDouble() : 0);
        });
        auto* orientation = new QWidget;
        auto* orientationLayout = new QGridLayout(orientation);
        orientationLayout->setContentsMargins(0, 8, 0, 0);
        const QStringList orientationLabels{tr("Rotate left"), tr("Rotate right"),
                                            tr("Flip horizontal"), tr("Flip vertical")};
        for (int i = 0; i < orientationLabels.size(); ++i) {
            auto* button = new QPushButton(orientationLabels[i]);
            bindTranslation(button, [this, button, i] {
                const QStringList labels{tr("Rotate left"), tr("Rotate right"), tr("Flip horizontal"), tr("Flip vertical")};
                button->setText(labels[i]);
            });
            button->setObjectName("secondary");
            orientationLayout->addWidget(button, i / 2, i % 2);
            connect(button, &QPushButton::clicked, this,
                    [this, i] { emit cropOrientationChanged(i); });
        }
        body_->addWidget(orientation);
        auto* note = new QLabel(tr("Straighten"));
        bindTranslation(note, [this, widget = note] { widget->setText(tr("Straighten")); });
        note->setObjectName("sectionCaption");
        note->setContentsMargins(0, 12, 0, 0);
        body_->addWidget(note);
        QDoubleSpinBox* angle = nullptr;
        body_->addWidget(row(
            QT_TR_NOOP("Angle"), -45, 45, .1, QString::fromUtf8("°"),
            [this](double v) { emit cropAngleChanged(v); }, &angle));
        cropAngleEditor_ = angle;
        auto* angleSlider = qobject_cast<QSlider*>(angle->property("slider").value<QObject*>());
        connect(angleSlider, &QSlider::sliderPressed, this, &ToolPanel::cropAngleGestureStarted);
        connect(angleSlider, &QSlider::valueChanged, this, [this, angleSlider, angle] {
            if (angleSlider->isSliderDown()) emit cropAngleChanged(angle->value());
        });
        connect(angleSlider, &QSlider::sliderReleased, this, &ToolPanel::cropAngleGestureFinished);
        setNumber(angle, valueAt(params_, "geometry.angle").toDouble());
        auto* horizon = new QPushButton(tr("Draw horizon"));
        bindTranslation(horizon, [this, widget = horizon] { widget->setText(tr("Draw horizon")); });
        bindTranslation(horizon, [this, widget = horizon] { widget->setToolTip(tr("Drag along a horizontal or vertical edge in the photo.")); });
        body_->addWidget(horizon);
        connect(horizon, &QPushButton::clicked, this, &ToolPanel::horizonRequested);
        auto* guides = new QComboBox;
        bindTranslation(guides, [this, widget = guides] { widget->setAccessibleName(tr("Crop guides")); });
        guides->addItems({tr("Rule of thirds"), tr("Center cross"), tr("No guides")});
        bindTranslation(guides, [this, guides] {
            const QSignalBlocker blocker(guides);
            const QStringList labels{tr("Rule of thirds"), tr("Center cross"), tr("No guides")};
            for (int i = 0; i < labels.size(); ++i) guides->setItemText(i, labels[i]);
        });
        auto* guideLabel = new QLabel;
        bindTranslation(guideLabel, [this, guideLabel] { guideLabel->setText(tr("Guides")); });
        body_->addWidget(guideLabel);
        body_->addWidget(guides);
        connect(guides, qOverload<int>(&QComboBox::activated), this, &ToolPanel::cropGuideChanged);
        emit cropGuideChanged(0);
        auto* help = new QLabel(tr("Drag inside to move the crop; drag a corner or edge grip to "
                                   "resize. Drag outside to rotate. Double-click inside to apply."));
        bindTranslation(help, [this, help] {
            help->setText(tr("Drag inside to move the crop; drag a corner or edge grip to "
                             "resize. Drag outside to rotate. Double-click inside to apply."));
        });
        help->setWordWrap(true);
        help->setObjectName("muted");
        help->setContentsMargins(0, 12, 0, 12);
        body_->addWidget(help);
        auto* actions = new QWidget;
        auto* al = new QHBoxLayout(actions);
        al->setContentsMargins(0, 0, 0, 0);
        auto* cancel = new QPushButton(tr("Cancel"));
        bindTranslation(cancel, [this, widget = cancel] { widget->setText(tr("Cancel")); });
        cancel->setObjectName("secondary");
        auto* apply = new QPushButton(tr("Apply crop"));
        bindTranslation(apply, [this, widget = apply] { widget->setText(tr("Apply crop")); });
        apply->setObjectName("primary");
        al->addWidget(cancel, 1);
        al->addWidget(apply, 1);
        body_->addWidget(actions);
        connect(cancel, &QPushButton::clicked, this, [this] {
            emit cropCancelled();
            emit done();
        });
        connect(apply, &QPushButton::clicked, this, [this] {
            emit cropAccepted();
            emit done();
        });
    } else {
        auto* creators = new QWidget;
        auto* cl = new QHBoxLayout(creators);
        cl->setContentsMargins(0, 0, 0, 0);
        cl->setSpacing(4);
        for (const auto& pair : std::array<std::pair<QString, QString>, 3>{
                 {{"Brush", "brush"}, {"Linear", "linear"}, {"Radial", "radial"}}}) {
            auto* b = new QPushButton(uiText(pair.first));
            bindTranslation(b, [b, source = pair.first] { b->setText(uiText(source)); });
            b->setObjectName("secondary");
            cl->addWidget(b, 1);
            connect(b, &QPushButton::clicked, this,
                    [this, pair] { emit toolSelected(pair.second); });
        }
        body_->addWidget(creators);
        masks_ = new QListWidget;
        masks_->setObjectName("maskList");
        masks_->setFixedHeight(128);
        masks_->setItemDelegate(new MaskDelegate(masks_));
        body_->addWidget(masks_);
        connect(masks_, &QListWidget::currentRowChanged, this, [this](int i) {
            selectMask(i);
            emit toolSelected("mask_edit");
        });
        connect(masks_, &QListWidget::itemDoubleClicked, this, [this](QListWidgetItem*) {
            auto values = params_["masks"].toArray();
            if (selected_ >= values.size())
                return;
            const auto name =
                QInputDialog::getText(this, tr("Rename mask"), tr("Name"), QLineEdit::Normal,
                                      values[selected_].toObject()["name"].toString());
            if (!name.trimmed().isEmpty())
                changeMask("name", name.trimmed());
        });
        auto* rangeRow = new QWidget;
        auto* rangeLayout = new QHBoxLayout(rangeRow);
        rangeLayout->setContentsMargins(0, 0, 0, 0);
        auto* range = new QPushButton(tr("+ Range mask"));
        bindTranslation(range, [this, widget = range] { widget->setText(tr("+ Range mask")); });
        range->setObjectName("secondary");
        auto* combine = new QPushButton(tr("Combine…"));
        bindTranslation(combine, [this, widget = combine] { widget->setText(tr("Combine…")); });
        rangeLayout->addWidget(range);
        auto* colorRange = new QPushButton(tr("+ Color mask"));
        bindTranslation(colorRange, [this, widget = colorRange] { widget->setText(tr("+ Color mask")); });
        colorRange->setObjectName("createColorMask");
        bindTranslation(colorRange, [this, widget = colorRange] { widget->setToolTip(tr("Click the photo to sample the color to select.")); });
        rangeLayout->addWidget(colorRange);
        connect(colorRange, &QPushButton::clicked, this, [this] { emit toolSelected("color_range"); });
        rangeLayout->addStretch();
        rangeLayout->addWidget(combine);
        body_->addWidget(rangeRow);
        connect(range, &QPushButton::clicked, this, [this] {
            QDialog dialog(this);
            dialog.setWindowTitle(tr("Luminance range mask"));
            auto* form = new QFormLayout(&dialog);
            auto* low = new QDoubleSpinBox;
            auto* high = new QDoubleSpinBox;
            for (auto* value : {low, high}) {
                value->setRange(0, 1);
                value->setSingleStep(.01);
                value->setDecimals(2);
            }
            high->setValue(1);
            auto* feather = new QDoubleSpinBox;
            feather->setRange(0, 100);
            feather->setValue(20);
            form->addRow(tr("Low luminance"), low);
            form->addRow(tr("High luminance"), high);
            form->addRow(tr("Smoothness (%)"), feather);
            auto* buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel);
            form->addRow(buttons);
            connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
            connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
            if (dialog.exec() != QDialog::Accepted)
                return;
            auto values = params_["masks"].toArray();
            qint64 id = 0;
            for (const auto& m : values)
                id = std::max(id, m.toObject()["id"].toInteger());
            values.append(QJsonObject{
                {"id", id + 1},
                {"name", tr("Luminance range %1").arg(id + 1)},
                {"invert", false},
                {"opacity", 1.0},
                {"adjust", QJsonObject{}},
                {"shape",
                 QJsonObject{
                     {"kind", "range"},
                     {"range", QJsonObject{{"kind", "luminance"},
                                           {"low", std::min(low->value(), high->value())},
                                           {"high", std::max(low->value(), high->value())}}},
                     {"smoothness", feather->value()}}}});
            selected_ = int(values.size()) - 1;
            publishMasks(values, tr("Add luminance range mask"));
        });
        connect(combine, &QPushButton::clicked, this, [this] {
            auto values = params_["masks"].toArray();
            if (selected_ < 0 || selected_ >= values.size() || values.size() < 2)
                return;
            QDialog dialog(this);
            dialog.setWindowTitle(tr("Combine mask coverage"));
            auto* form = new QFormLayout(&dialog);
            auto* operation = new QComboBox;
            operation->addItems({tr("Add"), tr("Subtract"), tr("Intersect")});
            auto* other = new QComboBox;
            for (int i = 0; i < values.size(); ++i)
                if (i != selected_)
                    other->addItem(values[i].toObject()["name"].toString(), i);
            form->addRow(tr("Operation"), operation);
            form->addRow(tr("Mask"), other);
            auto* note = new QLabel(
                tr("Copies current coverage. The other mask's adjustments remain independent."));
            note->setWordWrap(true);
            form->addRow(note);
            auto* buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel);
            form->addRow(buttons);
            connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
            connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
            if (dialog.exec() != QDialog::Accepted)
                return;
            const auto coverage = [](const QJsonObject& mask) -> QJsonObject {
                const auto shape = mask["shape"].toObject();
                if (!mask["invert"].toBool())
                    return shape;
                return {
                    {"kind", "combine"},
                    {"op", "subtract"},
                    {"parts", QJsonArray{QJsonObject{{"kind", "range"},
                                                     {"range", QJsonObject{{"kind", "luminance"},
                                                                           {"low", 0.0},
                                                                           {"high", 1.0}}},
                                                     {"smoothness", 0.0}},
                                         shape}}};
            };
            auto mask = values[selected_].toObject();
            const QJsonObject shape{
                {"kind", "combine"},
                {"op", QStringList{"add", "subtract", "intersect"}[operation->currentIndex()]},
                {"parts", QJsonArray{coverage(mask),
                                     coverage(values[other->currentData().toInt()].toObject())}}};
            std::function<int(QJsonObject)> depth = [&depth](QJsonObject node) {
                if (node["kind"] != "combine")
                    return 1;
                int used = 1, i = 0;
                for (const auto& part : node["parts"].toArray())
                    used = std::max(used, depth(part.toObject()) + (i++ > 0 ? 1 : 0));
                return used;
            };
            if (depth(shape) > 8) {
                QMessageBox::warning(
                    this, tr("Combine masks"),
                    tr("This combination exceeds the renderer's eight-value stack."));
                return;
            }
            mask["shape"] = shape;
            mask["invert"] = false;
            values[selected_] = mask;
            publishMasks(values, tr("Combine mask coverage"));
            emit maskSelected({});
        });
        auto* switches = new QWidget;
        auto* sw = new QHBoxLayout(switches);
        sw->setContentsMargins(0, 0, 0, 0);
        overlay_ = new QCheckBox(tr("Show overlay"));
        bindTranslation(overlay_, [this, widget = overlay_] { widget->setText(tr("Show overlay")); });
        invert_ = new QCheckBox(tr("Invert"));
        bindTranslation(invert_, [this, widget = invert_] { widget->setText(tr("Invert")); });
        sw->addWidget(overlay_);
        sw->addStretch();
        sw->addWidget(invert_);
        body_->addWidget(switches);
        connect(invert_, &QCheckBox::toggled, this, [this](bool v) { changeMask("invert", v); });
        connect(overlay_, &QCheckBox::toggled, this, [this](bool on) {
            if (on)
                selectMask(selected_);
            else
                emit command(commands::Overlays{.overlays = OverlayPatch{.mask = std::optional<qint64>{}}});
        });
        const auto add = [this](const char* label, const QString& path, double low, double high,
                                double step, const QString& unit) {
            QDoubleSpinBox* spin = nullptr;
            body_->addWidget(row(
                label, low, high, step, unit,
                [this, path](double v) { changeMask(path, path == "opacity" ? v / 100 : v); },
                &spin));
            sync_.append([this, spin, path] {
                const auto values = params_["masks"].toArray();
                spin->parentWidget()->setEnabled(selected_ >= 0 && selected_ < values.size());
                if (path == "shape.feather")
                    spin->parentWidget()->setVisible(
                        selected_ >= 0 && selected_ < values.size() &&
                        values[selected_].toObject()["shape"].toObject()["kind"] == "radial");
                if (path.startsWith("shape.range.") || path == "shape.smoothness") {
                    const auto mask = selected_ >= 0 && selected_ < values.size() ? values[selected_] : QJsonValue();
                    const bool isRange = valueAt(mask, "shape.kind") == "range";
                    const bool color = valueAt(mask, "shape.range.kind") == "color";
                    spin->parentWidget()->setVisible(isRange && (path == "shape.smoothness" ||
                        (path == "shape.range.tolerance" ? color : !color)));
                }
                if (selected_ >= 0 && selected_ < values.size())
                    setNumber(spin, valueAt(values[selected_], path).toDouble() *
                                        (path == "opacity" ? 100 : 1));
            });
        };
        add(QT_TR_NOOP("Opacity"), "opacity", 0, 100, 1, "%");
        add(QT_TR_NOOP("Feather"), "shape.feather", 0, 100, 1, "%");
        add(QT_TR_NOOP("Range smoothness"), "shape.smoothness", 0, 100, 1, "%");
        add(QT_TR_NOOP("Low luminance"), "shape.range.low", 0, 1, .01, "");
        add(QT_TR_NOOP("High luminance"), "shape.range.high", 0, 1, .01, "");
        add(QT_TR_NOOP("Color tolerance"), "shape.range.tolerance", 0, 100, 1, "%");
        auto* sampleColor = new QPushButton(tr("Sample mask color on photo"));
        bindTranslation(sampleColor, [this, widget = sampleColor] { widget->setText(tr("Sample mask color on photo")); });
        sampleColor->setObjectName("sampleMaskColor");
        body_->addWidget(sampleColor);
        connect(sampleColor, &QPushButton::clicked, this, [this] { emit toolSelected("color_pick"); });
        sync_.append([this, sampleColor] {
            const auto values = params_["masks"].toArray();
            sampleColor->setVisible(selected_ >= 0 && selected_ < values.size() &&
                valueAt(values[selected_], "shape.range.kind") == "color");
        });
        auto* local = new QLabel(tr("Local adjustments"));
        bindTranslation(local, [this, widget = local] { widget->setText(tr("Local adjustments")); });
        local->setObjectName("sectionCaption");
        local->setContentsMargins(0, 12, 0, 6);
        body_->addWidget(local);
        const QList<QPair<const char*, QString>> localFields{
            {QT_TR_NOOP("Exposure"), "exposure"}, {QT_TR_NOOP("Temperature"), "temperature"},
            {QT_TR_NOOP("Tint"), "tint"}, {QT_TR_NOOP("Contrast"), "contrast"},
            {QT_TR_NOOP("Highlights"), "highlights"}, {QT_TR_NOOP("Shadows"), "shadows"},
            {QT_TR_NOOP("Whites"), "whites"}, {QT_TR_NOOP("Blacks"), "blacks"},
            {QT_TR_NOOP("Texture"), "texture"}, {QT_TR_NOOP("Clarity"), "clarity"},
            {QT_TR_NOOP("Dehaze"), "dehaze"}, {QT_TR_NOOP("Saturation"), "saturation"},
            {QT_TR_NOOP("Hue"), "hue"}, {QT_TR_NOOP("Sharpness"), "sharpness"}, {QT_TR_NOOP("Noise"), "noise"}};
        for (const auto& pair : localFields) {
            const auto extent = pair.second == "exposure" ? 4 : pair.second == "hue" ? 180 : 100;
            add(pair.first, "adjust." + pair.second, -extent, extent,
                pair.second == "exposure" ? .01 : 1,
                pair.second == "exposure" ? "EV" : pair.second == "hue" ? tr("deg") : "");
        }
        auto* actions = new QWidget;
        auto* al = new QHBoxLayout(actions);
        al->setContentsMargins(0, 8, 0, 0);
        auto* copy = new QPushButton(glyph("copy"), tr("Duplicate"));
        auto* remove = new QPushButton(glyph("delete"), tr("Delete"));
        bindTranslation(copy, [this, copy] { copy->setText(tr("Duplicate")); });
        bindTranslation(remove, [this, remove] { remove->setText(tr("Delete")); });
        al->addWidget(copy);
        al->addStretch();
        al->addWidget(remove);
        body_->addWidget(actions);
        connect(copy, &QPushButton::clicked, this, [this] {
            auto values = params_["masks"].toArray();
            if (selected_ < 0 || selected_ >= values.size())
                return;
            auto mask = values[selected_].toObject();
            qint64 id = 0;
            for (const auto& m : values)
                id = std::max(id, m.toObject()["id"].toInteger());
            mask["id"] = id + 1;
            mask["name"] = mask["name"].toString() + tr(" copy");
            values.append(mask);
            publishMasks(values, tr("Duplicate mask"));
        });
        connect(remove, &QPushButton::clicked, this, [this] {
            auto values = params_["masks"].toArray();
            if (selected_ >= 0 && selected_ < values.size()) {
                values.removeAt(selected_);
                publishMasks(values, tr("Delete mask"));
                emit maskSelected({});
            }
        });
    }
    body_->addStretch();
    setParams(params_);
}
} // namespace aigle
