// Importing the design system loads its tokens, component classes and bundled fonts.
import "./index.css";

export { cx } from "./cx";
export { useFocusTrap, focusables, arrowIndex } from "./focus";
export { activateOnKey } from "./keys";

export { Icon, type IconProps } from "./components/core/Icon";
export { ICONS, ICON_NAMES, type IconName } from "./components/core/icons";
export { Button, type ButtonProps } from "./components/core/Button";
export { IconButton, type IconButtonProps } from "./components/core/IconButton";
export { Kbd, type KbdProps } from "./components/core/Kbd";

export { Input, type InputProps } from "./components/forms/Input";
export { Select, type SelectProps, type SelectOption } from "./components/forms/Select";
export { Combobox, type ComboboxProps, type ComboItem } from "./components/forms/Combobox";
export { Checkbox, type CheckboxProps } from "./components/forms/Checkbox";
export { Switch, type SwitchProps } from "./components/forms/Switch";
export { FolderPicker, splitPath, type FolderPickerProps, type FsEntry, type FolderListing, type FolderLister } from "./components/forms/FolderPicker";

export { Chip, type ChipProps, type Tone } from "./components/feedback/Chip";
export { StatusChip, statusTone, type StatusChipProps, type StatusKind } from "./components/feedback/StatusChip";
export { StatusDot, type StatusDotProps } from "./components/feedback/StatusDot";
export { Spinner, type SpinnerProps } from "./components/feedback/Spinner";
export { Banner, type BannerProps } from "./components/feedback/Banner";
export { Dialog, type DialogProps } from "./components/feedback/Dialog";

export { Tabs, type TabsProps, type TabItem } from "./components/navigation/Tabs";
export { TreeItem, type TreeItemProps } from "./components/navigation/TreeItem";
export { TreeSection, type TreeSectionProps } from "./components/navigation/TreeSection";
export { Breadcrumbs, type BreadcrumbsProps, type Crumb } from "./components/navigation/Breadcrumbs";
export { Menu, type MenuProps, type MenuItem, type MenuActionItem } from "./components/navigation/Menu";
export { Stepper, type StepperProps, type StepItem } from "./components/navigation/Stepper";
export { CommandPalette, filterPaletteItems, type CommandPaletteProps, type PaletteItem } from "./components/navigation/CommandPalette";

export { Panel, type PanelProps } from "./components/data/Panel";
export { SectionLabel, type SectionLabelProps } from "./components/data/SectionLabel";
export { Table, type TableProps, type TableColumn } from "./components/data/Table";
export { CodeView, colorLine, type CodeViewProps } from "./components/data/CodeView";

export { LaneStepper, LANE_ORDER, type LaneStepperProps, type LaneId, type LaneState } from "./components/pipeline/LaneStepper";
export { StageRow, type StageRowProps, type StageStatus } from "./components/pipeline/StageRow";
export { PhaseNode, type PhaseNodeProps } from "./components/pipeline/PhaseNode";
export { PhaseDag, type PhaseDagProps } from "./components/pipeline/PhaseDag";
export { ToolCall, type ToolCallProps, type PolicyInfo } from "./components/pipeline/ToolCall";
export { DiffView, type DiffViewProps, type DiffLine } from "./components/pipeline/DiffView";
export { GateCard, type GateCardProps, type GateKind } from "./components/pipeline/GateCard";
export { Decision, type DecisionProps } from "./components/pipeline/Decision";
export { ExecutionGroup, type ExecutionGroupProps, type ExecutionRun, type ExecutionRunStatus } from "./components/pipeline/ExecutionGroup";
export { Terminal, type TerminalProps, type TerminalLine, type TerminalTone } from "./components/pipeline/Terminal";
